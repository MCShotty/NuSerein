//! Offline socket regressions for bounded recovery of an established session.
use super::{tests::*, *};
use serde_json::json;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::{
	WebSocketStream, accept_async,
	tungstenite::protocol::{CloseFrame, frame::coding::CloseCode},
};

struct Gateway {
	initial: TcpListener,
	resume: TcpListener,
	reconnect: Notify,
	finished: Notify,
	manual: AtomicBool,
}

impl Gateway {
	async fn new() -> Self {
		Self {
			initial: TcpListener::bind("127.0.0.1:0").await.unwrap(),
			resume: TcpListener::bind("127.0.0.1:0").await.unwrap(),
			reconnect: Notify::new(),
			finished: Notify::new(),
			manual: AtomicBool::new(false),
		}
	}

	async fn run(&self, server: impl std::future::Future<Output = ()>) {
		timeout(Duration::from_secs(10), async {
			let initial = format!("ws://{}/", self.initial.local_addr().unwrap());
			let resume = format!("ws://{}/", self.resume.local_addr().unwrap());
			let client = async {
				let result = run_recoverable(
					Arc::new(
						SessionSecret::from_owner_input("synthetic-recovery-secret".into())
							.unwrap(),
					),
					"wss://gateway.discord.gg/".into(),
					watch::channel(None).1,
					mpsc::channel(1).1,
					None,
					Some(&self.reconnect),
					|event| {
						if matches!(event, Event::Disconnected)
							&& !self.manual.load(Ordering::Relaxed)
						{
							self.reconnect.notify_one();
						}
						Ok(())
					},
					Some((&initial, &resume)),
				)
				.await;
				self.finished.notify_one();
				result
			};
			let ((), result) = tokio::join!(server, client);
			assert_eq!(result, Err(Failure::Expired));
		})
		.await
		.expect("synthetic gateway recovery exceeded its bounded deadline");
	}

	async fn socket(&self, resume: bool, op: Option<u64>) -> WebSocketStream<TcpStream> {
		let (stream, resumed) = tokio::select! {
			result = self.initial.accept() => (result.unwrap().0, false),
			result = self.resume.accept() => (result.unwrap().0, true),
		};
		assert_eq!(resumed, resume, "recovery selected the wrong gateway URL");
		stream.set_nodelay(true).unwrap();
		let mut socket = accept_async(stream).await.unwrap();
		if let Some(op) = op {
			send(
				&mut socket,
				json!({"op":10,"d":{"heartbeat_interval":30000}}),
			)
			.await;
			let handshake = packet(&mut socket).await;
			assert_eq!(handshake["op"], op);
			if op == 6 {
				assert_eq!(handshake["d"]["session_id"], "synthetic-recovery-session");
			} else {
				send(&mut socket, json!({"op":1,"d":null})).await;
				let heartbeat = packet(&mut socket).await;
				assert_eq!(heartbeat["op"], 1);
				assert!(
					heartbeat["d"].is_null(),
					"Identify must clear the resume sequence"
				);
				send(&mut socket, json!({"op":11,"d":null})).await;
			}
		}
		socket
	}

	async fn establish(&self) {
		let mut socket = self.socket(false, Some(2)).await;
		let mut ready = ready(41, "synthetic-recovery-session");
		ready["d"]["resume_gateway_url"] = json!("wss://gateway-resume.discord.gg/");
		send(&mut socket, ready).await;
		acknowledge(&mut socket, 41).await;
	}

	async fn stop(&self, mut socket: WebSocketStream<TcpStream>) {
		close(&mut socket, 4004).await;
		tokio::select! {
			() = self.finished.notified() => {},
			_ = self.initial.accept() => panic!("terminal close must not reconnect"),
			_ = self.resume.accept() => panic!("terminal close must not resume"),
		}
	}
}

async fn close(socket: &mut WebSocketStream<TcpStream>, code: u16) {
	socket
		.send(Frame::Close(Some(CloseFrame {
			code: CloseCode::from(code),
			reason: "synthetic close".into(),
		})))
		.await
		.unwrap();
}

#[tokio::test]
async fn three_failed_resumes_identify_at_original_gateway() {
	let gateway = Gateway::new().await;
	gateway
		.run(async {
			gateway.establish().await;
			drop(gateway.socket(true, None).await);
			for _ in 0..2 {
				drop(gateway.socket(true, Some(6)).await);
			}
			gateway.stop(gateway.socket(false, Some(2)).await).await;
		})
		.await;
}

#[tokio::test]
async fn resumed_session_resets_consecutive_failure_budget() {
	let gateway = Gateway::new().await;
	gateway
		.run(async {
			gateway.establish().await;
			for _ in 0..2 {
				drop(gateway.socket(true, Some(6)).await);
			}
			let mut socket = gateway.socket(true, Some(6)).await;
			send(&mut socket, json!({"op":0,"t":"RESUMED","s":42,"d":{}})).await;
			acknowledge(&mut socket, 42).await;
			drop(socket);
			for _ in 0..3 {
				drop(gateway.socket(true, Some(6)).await);
			}
			gateway.stop(gateway.socket(false, Some(2)).await).await;
		})
		.await;
}

#[tokio::test]
async fn manual_recovery_does_not_exhaust_resume_budget() {
	let gateway = Gateway::new().await;
	gateway
		.run(async {
			gateway.establish().await;
			let mut socket = gateway.socket(true, None).await;
			gateway.manual.store(true, Ordering::Relaxed);
			for op in [Some(6), None, Some(6), Some(6)] {
				gateway.reconnect.notify_one();
				let next = gateway.socket(true, op).await;
				drop(socket);
				socket = next;
			}
			gateway.stop(socket).await;
		})
		.await;
}

#[tokio::test]
async fn expired_session_before_hello_stops_recovery() {
	let gateway = Gateway::new().await;
	gateway
		.run(async {
			gateway.establish().await;
			gateway.stop(gateway.socket(true, None).await).await;
		})
		.await;
}

#[tokio::test]
async fn invalid_session_before_hello_identifies_at_original_gateway() {
	let gateway = Gateway::new().await;
	gateway
		.run(async {
			gateway.establish().await;
			let mut socket = gateway.socket(true, None).await;
			close(&mut socket, 4009).await;
			let next = gateway.socket(false, Some(2)).await;
			drop(socket);
			gateway.stop(next).await;
		})
		.await;
}

#[tokio::test]
async fn recovery_probes_a_ready_socket_and_resumes_only_when_it_is_silent() {
	let gateway = Gateway::new().await;
	gateway
		.run(async {
			let mut socket = gateway.socket(false, Some(2)).await;
			let mut ready = ready(41, "synthetic-recovery-session");
			ready["d"]["resume_gateway_url"] = json!("wss://gateway-resume.discord.gg/");
			send(&mut socket, ready).await;
			acknowledge(&mut socket, 41).await;
			gateway.manual.store(true, Ordering::Relaxed);
			// Timer/requested heartbeats and recovery probes have the same wire payload.
			// Deliberately race a requested heartbeat with the probe and acknowledge both.
			send(&mut socket, json!({"op":1,"d":null})).await;
			gateway.reconnect.notify_one();
			let healthy_until = Instant::now() + PROBE_TIMEOUT * 2;
			let mut heartbeats = 0;
			loop {
				tokio::select! {
					_ = tokio::time::sleep_until(healthy_until) => break,
					packet = packet(&mut socket) => {
						assert_eq!((packet["op"].clone(), packet["d"].clone()), (json!(1), json!(41)));
						send(&mut socket, json!({"op":11,"d":null})).await;
						heartbeats += 1;
					}
					_ = gateway.initial.accept() => panic!("healthy probe must not reconnect"),
					_ = gateway.resume.accept() => panic!("healthy probe must not resume"),
				}
			}
			assert!(
				heartbeats >= 2,
				"requested and probe heartbeats must both arrive"
			);
			// The ordered Pong proves the client consumed every preceding ACK. Otherwise a
			// delayed ACK can clear the silent probe and leave this test waiting for a resume.
			let barrier = b"recovery-ack-barrier";
			socket
				.send(Frame::Ping(barrier.as_slice().into()))
				.await
				.unwrap();
			loop {
				match socket.next().await.unwrap().unwrap() {
					Frame::Pong(data) => {
						assert_eq!(data.as_ref(), barrier);
						break;
					}
					Frame::Text(text) => {
						let packet: serde_json::Value = serde_json::from_str(&text).unwrap();
						assert_eq!(packet["op"], 1);
					}
					other => panic!("unexpected frame at recovery ACK barrier: {other:?}"),
				}
			}
			let silent_since = Instant::now();
			gateway.reconnect.notify_one();
			let probe = packet(&mut socket).await;
			assert_eq!(
				(probe["op"].clone(), probe["d"].clone()),
				(json!(1), json!(41))
			);
			let next = gateway.socket(true, Some(6)).await;
			assert!(silent_since.elapsed() >= PROBE_TIMEOUT);
			drop(socket);
			gateway.stop(next).await;
		})
		.await;
}

#[tokio::test]
async fn server_requested_reconnect_resumes_without_the_failure_backoff() {
	let gateway = Gateway::new().await;
	gateway.manual.store(true, Ordering::Relaxed);
	gateway
		.run(async {
			let mut socket = gateway.socket(false, Some(2)).await;
			let mut ready = ready(41, "synthetic-recovery-session");
			ready["d"]["resume_gateway_url"] = json!("wss://gateway-resume.discord.gg/");
			send(&mut socket, ready).await;
			acknowledge(&mut socket, 41).await;
			let started = Instant::now();
			send(&mut socket, json!({"op":7,"d":null})).await;
			let next = timeout(Duration::from_secs(1), gateway.socket(true, Some(6)))
				.await
				.expect("server migration must not wait for failure backoff");
			assert!(started.elapsed() < Duration::from_secs(1));
			gateway.stop(next).await;
		})
		.await;
}

/// Poll real loopback I/O while keeping the paused runtime runnable. Advancing the
/// clock remains explicit: ordinary packet timeouts cannot advance the test for us.
async fn buffered_packets(socket: &mut WebSocketStream<TcpStream>) -> Vec<serde_json::Value> {
	let mut packets = Vec::new();
	for _ in 0..400 {
		tokio::task::yield_now().await;
		if let Some(frame) = socket.next().now_or_never() {
			match frame.expect("synthetic socket closed").unwrap() {
				Frame::Text(text) => {
					let value: serde_json::Value = serde_json::from_str(&text).unwrap();
					if value["op"] == 1 {
						send(socket, json!({"op":11,"d":null})).await;
					} else {
						packets.push(value);
					}
				}
				Frame::Pong(_) => {}
				other => panic!("unexpected synthetic frame: {other:?}"),
			}
		}
		assert!(packets.len() < 32);
	}
	packets
}

#[tokio::test]
async fn member_subscription_backoff_is_preserved_by_the_production_send_loop() {
	let initial = TcpListener::bind("127.0.0.1:0").await.unwrap();
	let endpoint = format!("ws://{}/", initial.local_addr().unwrap());
	let (_subscriptions, subscriptions) = watch::channel(Some(MemberSubscription {
		thread: false,
		guild: Id(2),
		channel: Id(3),
		request: 1,
		list_id: "everyone".into(),
		ranges: vec![[0, 99]],
	}));
	let client = run_recoverable(
		Arc::new(SessionSecret::from_owner_input("synthetic-retry-secret".into()).unwrap()),
		"wss://gateway.discord.gg/".into(),
		subscriptions,
		mpsc::channel(1).1,
		None,
		None,
		|_| Ok(()),
		Some((&endpoint, &endpoint)),
	);
	let server = async {
		let (stream, _) = initial.accept().await.unwrap();
		stream.set_nodelay(true).unwrap();
		let mut socket = accept_async(stream).await.unwrap();
		send(
			&mut socket,
			json!({"op":10,"d":{"heartbeat_interval":120000}}),
		)
		.await;
		assert_eq!(packet(&mut socket).await["op"], 2);
		send(&mut socket, ready(1, "synthetic-retry-session")).await;
		let mut initial_subscriptions = 0;
		while initial_subscriptions < 2 {
			let value = packet(&mut socket).await;
			if value["op"] == 1 {
				send(&mut socket, json!({"op":11,"d":null})).await;
			} else {
				assert_eq!(value["op"], 37);
				initial_subscriptions += 1;
			}
		}
		for _ in 0..100 {
			tokio::task::yield_now().await;
		}
		tokio::time::pause();
		for delay in [15, 30, 60, 120] {
			tokio::time::advance(Duration::from_secs(delay - 1)).await;
			assert!(
				buffered_packets(&mut socket).await.is_empty(),
				"member retry fired before its increasing deadline"
			);
			tokio::time::advance(Duration::from_secs(1)).await;
			// Tokio rounds timer-wheel deadlines up to a millisecond. Cross that
			// resolution explicitly without changing the intended retry intervals.
			tokio::time::advance(Duration::from_millis(1)).await;
			let retry = buffered_packets(&mut socket).await;
			assert_eq!(
				retry.len(),
				2,
				"each retry must reset then resubscribe, once"
			);
			assert!(retry.iter().all(|value| value["op"] == 37));
			assert_eq!(
				retry[1]["d"]["subscriptions"]["2"]["channels"],
				json!({"3":[[0,99]]})
			);
		}
		tokio::time::resume();
		close(&mut socket, 4004).await;
		// Retain TCP until the client reads Close, avoiding an unread-heartbeat reset race.
		for _ in 0..100 {
			tokio::task::yield_now().await;
		}
	};
	let (result, ()) = tokio::join!(client, server);
	assert_eq!(result, Err(Failure::Expired));
}

#[tokio::test]
async fn buffered_ack_survives_a_scheduler_pause_without_replacing_the_socket() {
	let initial = TcpListener::bind("127.0.0.1:0").await.unwrap();
	let endpoint = format!("ws://{}/", initial.local_addr().unwrap());
	let disconnected = std::sync::atomic::AtomicUsize::new(0);
	let client = run_recoverable(
		Arc::new(SessionSecret::from_owner_input("synthetic-paused-heartbeat".into()).unwrap()),
		"wss://gateway.discord.gg/".into(),
		watch::channel(None).1,
		mpsc::channel(1).1,
		None,
		None,
		|event| {
			if matches!(event, Event::Disconnected) {
				disconnected.fetch_add(1, Ordering::Relaxed);
			}
			Ok(())
		},
		Some((&endpoint, &endpoint)),
	);
	let server = async {
		let (stream, _) = initial.accept().await.unwrap();
		stream.set_nodelay(true).unwrap();
		let mut socket = accept_async(stream).await.unwrap();
		send(
			&mut socket,
			json!({"op":10,"d":{"heartbeat_interval":1000}}),
		)
		.await;
		assert_eq!(packet(&mut socket).await["op"], 2);
		send(&mut socket, ready(1, "synthetic-paused-session")).await;
		let heartbeat = packet(&mut socket).await;
		assert_eq!(heartbeat["op"], 1);
		for _ in 0..20 {
			tokio::task::yield_now().await;
		}
		tokio::time::pause();
		for burst in 0..20 {
			// Real data already queued ahead of an ACK must be drained in bounded
			// batches after the scheduler resumes, without declaring a false loss.
			for offset in 0..128 {
				send(
					&mut socket,
					json!({"op":0,"t":"SYNTHETIC_UNSUPPORTED","s":2+burst*128+offset,"d":{}}),
				)
				.await;
			}
			// Queue the ACK before resuming a timer which is already overdue.
			send(&mut socket, json!({"op":11,"d":null})).await;
			tokio::time::advance(Duration::from_secs(2)).await;
			assert!(buffered_packets(&mut socket).await.is_empty());
			assert_eq!(
				disconnected.load(Ordering::Relaxed),
				0,
				"buffered ACK lost to expired timer"
			);
		}
		tokio::time::resume();
		close(&mut socket, 4004).await;
		for _ in 0..100 {
			tokio::task::yield_now().await;
		}
	};
	let (result, ()) = tokio::join!(client, server);
	assert_eq!(result, Err(Failure::Expired));
}

#[tokio::test]
async fn gateway_rate_limit_reports_disconnect_immediately_and_manual_recovery_cannot_skip_cooldown()
 {
	let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
	let endpoint = format!("ws://{}/", listener.local_addr().unwrap());
	let disconnected = AtomicBool::new(false);
	let reconnect = Notify::new();
	let diagnostics = Arc::new(Mutex::new(ConnectionDiagnostics::default()));
	let client = run_observed(
		Arc::new(SessionSecret::from_owner_input("synthetic-rate-secret".into()).unwrap()),
		"wss://gateway.discord.gg/".into(),
		watch::channel(None).1,
		mpsc::channel(1).1,
		None,
		Some(&reconnect),
		diagnostics.clone(),
		|event| {
			if matches!(event, Event::Disconnected) {
				disconnected.store(true, Ordering::Relaxed);
			}
			Ok(())
		},
		Some((&endpoint, &endpoint)),
	);
	let server = async {
		let (stream, _) = listener.accept().await.unwrap();
		stream.set_nodelay(true).unwrap();
		let mut first = accept_async(stream).await.unwrap();
		send(
			&mut first,
			json!({"op":10,"d":{"heartbeat_interval":120000}}),
		)
		.await;
		assert_eq!(packet(&mut first).await["op"], 2);
		send(&mut first, ready(1, "synthetic-rate-session")).await;
		acknowledge(&mut first, 1).await;
		close(&mut first, 4008).await;
		for _ in 0..1000 {
			if disconnected.load(Ordering::Relaxed) {
				break;
			}
			tokio::task::yield_now().await;
		}
		assert!(
			disconnected.load(Ordering::Relaxed),
			"cooldown must not leave the UI claiming connected"
		);
		assert_eq!(
			diagnostics.lock().unwrap().last_reason(),
			Some(ConnectionReason::RateLimited.label())
		);
		tokio::time::pause();
		for _ in 0..20 {
			reconnect.notify_one();
		}
		tokio::time::advance(Duration::from_secs(59)).await;
		for _ in 0..100 {
			tokio::task::yield_now().await;
		}
		assert!(
			listener.accept().now_or_never().is_none(),
			"manual recovery bypassed the Gateway cooldown"
		);
		tokio::time::advance(Duration::from_secs(2)).await;
		tokio::time::resume();
		let (stream, _) = timeout(Duration::from_secs(2), listener.accept())
			.await
			.unwrap()
			.unwrap();
		stream.set_nodelay(true).unwrap();
		let mut next = accept_async(stream).await.unwrap();
		send(
			&mut next,
			json!({"op":10,"d":{"heartbeat_interval":120000}}),
		)
		.await;
		assert_eq!(packet(&mut next).await["op"], 6);
		close(&mut next, 4004).await;
		for _ in 0..100 {
			tokio::task::yield_now().await;
		}
	};
	let (result, ()) = tokio::join!(client, server);
	assert_eq!(result, Err(Failure::Expired));
	let history = diagnostics.lock().unwrap().summary();
	assert!(history.contains("code=4008"));
	assert!(!history.contains("synthetic"));
}

#[tokio::test]
async fn protocol_errors_from_dispatch_are_retained_by_the_diagnostic_boundary() {
	let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
	let endpoint = format!("ws://{}/", listener.local_addr().unwrap());
	let diagnostics = Arc::new(Mutex::new(ConnectionDiagnostics::default()));
	let client = run_observed(
		Arc::new(SessionSecret::from_owner_input("synthetic-diagnostic-secret".into()).unwrap()),
		"wss://gateway.discord.gg/".into(),
		watch::channel(None).1,
		mpsc::channel(1).1,
		None,
		None,
		diagnostics.clone(),
		|_| Ok(()),
		Some((&endpoint, &endpoint)),
	);
	let server = async {
		let (stream, _) = listener.accept().await.unwrap();
		stream.set_nodelay(true).unwrap();
		let mut socket = accept_async(stream).await.unwrap();
		send(
			&mut socket,
			json!({"op":10,"d":{"heartbeat_interval":120000}}),
		)
		.await;
		assert_eq!(packet(&mut socket).await["op"], 2);
		send(&mut socket, ready(1, "synthetic-sensitive-session")).await;
		acknowledge(&mut socket, 1).await;
		send(
			&mut socket,
			json!({"op":99,"d":{"private":"SYNTHETIC_PRIVATE_PAYLOAD"}}),
		)
		.await;
		for _ in 0..100 {
			tokio::task::yield_now().await;
		}
	};
	let (result, ()) = tokio::join!(client, server);
	assert_eq!(result, Err(Failure::Protocol));
	let history = diagnostics.lock().unwrap();
	assert_eq!(
		history.last_reason(),
		Some(ConnectionReason::Protocol.label())
	);
	assert!(!history.summary().contains("SYNTHETIC"));
	assert!(!history.summary().contains("synthetic"));
}

#[tokio::test]
async fn split_reader_sends_one_automatic_pong_and_keeps_requested_heartbeats() {
	let gateway = Gateway::new().await;
	gateway
		.run(async {
			let mut socket = gateway.socket(false, Some(2)).await;
			send(&mut socket, ready(41, "synthetic-recovery-session")).await;
			acknowledge(&mut socket, 41).await;
			socket
				.send(Frame::Ping(b"synthetic-pong-barrier".as_slice().into()))
				.await
				.unwrap();
			send(&mut socket, json!({"op":1,"d":null})).await;
			let mut pongs = 0;
			let mut heartbeat = false;
			while pongs == 0 || !heartbeat {
				match timeout(Duration::from_secs(1), socket.next())
					.await
					.unwrap()
					.unwrap()
					.unwrap()
				{
					Frame::Pong(bytes) => {
						assert_eq!(bytes.as_ref(), b"synthetic-pong-barrier");
						pongs += 1;
						assert_eq!(pongs, 1);
					}
					Frame::Text(text) => {
						let value: serde_json::Value = serde_json::from_str(&text).unwrap();
						assert_eq!(value["op"], 1);
						send(&mut socket, json!({"op":11,"d":null})).await;
						heartbeat = true;
					}
					other => panic!("unexpected synthetic frame: {other:?}"),
				}
			}
			send(&mut socket, json!({"op":1,"d":null})).await;
			assert_eq!(
				packet(&mut socket).await["op"],
				1,
				"a duplicate Pong must not precede the next heartbeat"
			);
			send(&mut socket, json!({"op":11,"d":null})).await;
			gateway.stop(socket).await;
		})
		.await;
}
