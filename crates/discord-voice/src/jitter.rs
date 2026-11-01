//! Fixed 40ms startup with a bounded reorder window for 2.5–120 ms Opus packets.
const SLOTS: usize = 8;
// Preserve the original total payload budget while admitting multi-frame packets.
const QUEUE_BYTES: usize = SLOTS * 1275;
#[derive(Default)]
pub(crate) struct Jitter {
	packets: Vec<(u16, Vec<u8>, Option<u32>)>,
	next: Option<u16>,
	wait: u8,
	// The final wait tick reaches zero before the first packet is played.
	started: bool,
	missing: u8,
}
impl Jitter {
	pub fn clear(&mut self) {
		*self = Self::default();
	}
	pub fn push(&mut self, sequence: u16, opus: Vec<u8>) {
		self.push_timed(sequence, None, opus);
	}
	pub fn push_timed(&mut self, sequence: u16, timestamp: Option<u32>, opus: Vec<u8>) {
		if opus.len() > crate::crypto::MAX_PACKET {
			return;
		}
		if self.next.is_none() {
			self.next = Some(sequence);
			self.wait = 2;
			self.started = false;
		}
		let next = self.next.unwrap();
		let distance = sequence.wrapping_sub(next);
		let earlier = distance >= 32768;
		if earlier
			&& (self.started
				|| next.wrapping_sub(sequence) >= SLOTS as u16
				|| self
					.packets
					.iter()
					.any(|(id, _, _)| id.wrapping_sub(sequence) >= SLOTS as u16))
		{
			return;
		}
		if !earlier && distance >= SLOTS as u16 {
			self.packets.clear();
			self.next = Some(sequence);
			self.wait = 2;
			self.started = false;
			self.missing = 0;
		}
		if self.packets.len() < SLOTS
			&& !self.packets.iter().any(|(id, _, _)| *id == sequence)
			&& self
				.packets
				.iter()
				.map(|(_, data, _)| data.len())
				.sum::<usize>()
				+ opus.len() <= QUEUE_BYTES
		{
			if earlier {
				self.next = Some(sequence);
			}
			self.packets.push((sequence, opus, timestamp));
		}
	}
	/// An empty packet requests Opus packet-loss concealment, at most three consecutive packets.
	#[cfg(test)]
	pub fn pop(&mut self) -> Option<Vec<u8>> {
		self.pop_timed().map(|(opus, _)| opus)
	}
	pub fn pop_timed(&mut self) -> Option<(Vec<u8>, Option<u32>)> {
		if self.wait > 0 {
			// Short packets can fill the fixed window before 40ms. Start before
			// the next arrival would repeatedly reset a full window.
			if self.packets.len() < SLOTS {
				self.wait -= 1;
				return None;
			}
			self.wait = 0;
		}
		let next = self.next?;
		self.started = true;
		self.next = Some(next.wrapping_add(1));
		if let Some(index) = self.packets.iter().position(|(id, _, _)| *id == next) {
			self.missing = 0;
			let (_, opus, timestamp) = self.packets.swap_remove(index);
			return Some((opus, timestamp));
		}
		self.missing += 1;
		if self.missing > 3 {
			// Stop concealing the gap, but retain speech that already arrived after it.
			if let Some((index, (sequence, _, _))) = self
				.packets
				.iter()
				.enumerate()
				.min_by_key(|(_, (sequence, _, _))| sequence.wrapping_sub(next))
			{
				self.next = Some(sequence.wrapping_add(1));
				self.missing = 0;
				let (_, opus, timestamp) = self.packets.swap_remove(index);
				return Some((opus, timestamp));
			}
			self.clear();
			return None;
		}
		Some((Vec::new(), None))
	}
}
#[cfg(test)]
mod tests {
	use super::*;
	#[test]
	fn startup_reorders_earlier_packets_without_rewinding_playback() {
		for first in [100_u16, u16::MAX] {
			let mut jitter = Jitter::default();
			jitter.push_timed(first.wrapping_add(1), Some(960), vec![2]);
			jitter.push_timed(first, Some(0), vec![1]);
			jitter.push_timed(first.wrapping_add(2), Some(1920), vec![3]);
			assert!(jitter.pop_timed().is_none());
			assert!(jitter.pop_timed().is_none());
			assert_eq!(jitter.pop_timed(), Some((vec![1], Some(0))));
			jitter.push_timed(first, Some(0), vec![9]);
			assert_eq!(jitter.pop_timed(), Some((vec![2], Some(960))));
			assert_eq!(jitter.pop_timed(), Some((vec![3], Some(1920))));
		}
	}
	#[test]
	fn startup_can_accept_earlier_packets_after_waiting_but_before_output() {
		let mut jitter = Jitter::default();
		jitter.push(12, vec![2]);
		assert!(jitter.pop().is_none());
		assert!(jitter.pop().is_none());
		jitter.push(11, vec![1]);
		assert_eq!(jitter.pop(), Some(vec![1]));
		assert_eq!(jitter.pop(), Some(vec![2]));
		jitter.push(100, vec![4]);
		jitter.push(99, vec![3]);
		assert!(jitter.pop().is_none());
		assert!(jitter.pop().is_none());
		assert_eq!(jitter.pop(), Some(vec![3]));
		assert_eq!(jitter.pop(), Some(vec![4]));
	}
	#[test]
	fn startup_rewind_keeps_the_existing_window_and_byte_limits() {
		let mut jitter = Jitter::default();
		jitter.push(100, vec![1]);
		jitter.push(107, vec![2]);
		jitter.push(99, vec![3]);
		assert_eq!(jitter.next, Some(100));
		assert_eq!(jitter.packets.len(), 2);

		jitter.clear();
		jitter.push(100, vec![1; crate::crypto::MAX_PACKET]);
		jitter.push(101, vec![2; crate::crypto::MAX_PACKET]);
		jitter.push(99, vec![3; crate::crypto::MAX_PACKET]);
		assert_eq!(jitter.next, Some(100));
		assert_eq!(jitter.packets.len(), 2);
	}
	#[test]
	fn timestamps_follow_reordered_packets_and_survive_sequence_wrap() {
		let mut jitter = Jitter::default();
		for (sequence, timestamp) in [(u16::MAX, u32::MAX - 959), (1, 960), (0, 0)] {
			jitter.push_timed(sequence, Some(timestamp), vec![1]);
		}
		assert!(jitter.pop_timed().is_none());
		assert!(jitter.pop_timed().is_none());
		for timestamp in [u32::MAX - 959, 0, 960] {
			assert_eq!(jitter.pop_timed(), Some((vec![1], Some(timestamp))));
		}
		assert_eq!(jitter.pop_timed(), Some((vec![], None)));
		jitter.clear();
		assert!(jitter.pop_timed().is_none());
	}
	#[test]
	fn burst_loss_preserves_buffered_recovery_and_resets_on_far_jump() {
		for first in [0u16, u16::MAX - 2] {
			let mut jitter = Jitter::default();
			jitter.push(first, vec![0]);
			for offset in [7, 5, 6] {
				jitter.push(first.wrapping_add(offset), vec![offset as u8]);
			}
			assert!(jitter.pop().is_none());
			assert!(jitter.pop().is_none());
			assert_eq!(jitter.pop(), Some(vec![0]));
			for _ in 0..3 {
				assert_eq!(jitter.pop(), Some(vec![]));
			}
			for offset in [5, 6, 7] {
				assert_eq!(jitter.pop(), Some(vec![offset]));
			}
			for _ in 0..2 {
				assert_eq!(jitter.pop(), Some(vec![]));
			}
			jitter.push(first.wrapping_add(100), vec![100]);
			assert_eq!(jitter.missing, 0);
			assert!(jitter.pop().is_none());
			assert!(jitter.pop().is_none());
			assert_eq!(jitter.pop(), Some(vec![100]));
		}
	}
	#[test]
	fn bounded_reordering_loss_duplicates_and_wrap() {
		let mut jitter = Jitter::default();
		jitter.push(u16::MAX, vec![1]);
		jitter.push(1, vec![3]);
		jitter.push(0, vec![2]);
		jitter.push(0, vec![9]);
		assert!(jitter.pop().is_none());
		assert!(jitter.pop().is_none());
		assert_eq!(jitter.pop(), Some(vec![1]));
		assert_eq!(jitter.pop(), Some(vec![2]));
		assert_eq!(jitter.pop(), Some(vec![3]));
		for _ in 0..3 {
			assert_eq!(jitter.pop(), Some(vec![]));
		}
		assert!(jitter.pop().is_none());
		assert!(jitter.pop().is_none());
		for sequence in 0..1000 {
			jitter.push(sequence, vec![1; 1275]);
			assert!(jitter.packets.len() <= SLOTS);
		}
		jitter.push(1000, vec![0; crate::crypto::MAX_PACKET + 1]);
		assert!(jitter.packets.len() <= SLOTS);
	}

	#[test]
	fn larger_packets_respect_the_existing_queue_byte_budget() {
		let mut jitter = Jitter::default();
		jitter.push(1, vec![0; crate::crypto::MAX_PACKET + 1]);
		assert!(jitter.next.is_none());
		for sequence in 1..=3 {
			jitter.push(sequence, vec![sequence as u8; crate::crypto::MAX_PACKET]);
		}
		assert_eq!(jitter.packets.len(), 2);
		assert!(
			jitter
				.packets
				.iter()
				.map(|(_, data, _)| data.len())
				.sum::<usize>()
				<= QUEUE_BYTES
		);
		assert!(jitter.pop().is_none());
		assert!(jitter.pop().is_none());
		assert_eq!(jitter.pop().unwrap().len(), crate::crypto::MAX_PACKET);
		jitter.push(3, vec![3; crate::crypto::MAX_PACKET]);
		assert_eq!(jitter.packets.len(), 2);
	}
}
