// Runs the shipping provider wrapper with an offline YouTube API double.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');
const html = fs.readFileSync('crates/platform/src/embed_player.html', 'utf8');
const script = html.match(/<script nonce="serein-player">([\s\S]*?)<\/script>/)[1]
  .replace('__VIDEO__', JSON.stringify({id:'synthet1c01', start:12}));
function fixture(visible = true) {
  const events = [], calls = [];
  let options;
  const api = {getCurrentTime:()=>4.5, pauseVideo(){calls.push('pause'); options.events.onStateChange({data:2});}, playVideo(){calls.push('play'); options.events.onStateChange({data:1});}};
  const window = {ipc:{postMessage:s=>events.push(JSON.parse(s))}};
  const YT = {Player:function(id, config) { assert.equal(id, 'player'); options=config; return api; }};
  vm.runInNewContext(script, {window, YT, location:{origin:'https://serein-player.localhost'}, setInterval(){}});
  window.onYouTubeIframeAPIReady();
  assert.equal(options.videoId,'synthet1c01');
  assert.equal(options.host,'https://www.youtube-nocookie.com');
  assert.equal(options.playerVars.start,12);
  if (visible) window.sereinVisibility(true);
  options.events.onReady();
  return {window,events,calls,options};
}
{
  const f=fixture();f.calls.length=0;
  f.window.sereinVisibility(false);f.window.sereinVisibility(false);
  assert.deepEqual(f.calls,['pause']);
  f.window.sereinVisibility(true);assert.deepEqual(f.calls,['pause','play']);
  assert.equal(f.events.at(-1).state,'playing');
}
for (const state of [0,2]) {
  const f=fixture();f.options.events.onStateChange({data:state});f.calls.length=0;
  f.window.sereinVisibility(false);f.window.sereinVisibility(true);
  assert.deepEqual(f.calls,['pause'],'manual pause/end must not auto-resume');
}
{
  const f=fixture();f.options.events.onAutoplayBlocked();f.calls.length=0;
  f.window.sereinVisibility(false);f.window.sereinVisibility(true);assert.deepEqual(f.calls,['pause']);
}
for (const error of [100,101,150,153]) {
  const f=fixture();f.options.events.onError({data:error});
  assert.equal(f.events.at(-1).state,'failed');assert.equal(f.events.at(-1).error,error);
  assert.deepEqual(Object.keys(f.events.at(-1)).sort(),['error','position','state']);
}
console.log('Provider player: offline API status, explicit errors, visibility and manual pause checks passed.');

{ const f=fixture(false);assert.equal(f.events.at(-1).state,'paused');f.calls.length=0;f.window.sereinVisibility(true);assert.deepEqual(f.calls,['play']); }
