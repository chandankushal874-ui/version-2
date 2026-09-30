/**
 * fix1_lang_gate.test.js -- Regression tests for FIX-1
 * Language gate in forwardOllalinkToRoom must not silently drop audio when
 * Ollalink omits the language field or peer has no targetLang.
 */

// ESM: set env vars BEFORE dynamic imports (static imports are hoisted)
process.env.PORT = '8790';
process.env.PUBLIC_BASE = 'ws://localhost:8790';
process.env.OLLALINK_DASHBOARD_KEY = 'sk_test';
process.env.OLLALINK_WS_URL = 'wss://example.com';
process.env.SESSION_SECRET = 'test-secret-32-bytes-long-padded!!';
process.env.LOG_LEVEL = 'error';

import { test } from 'node:test';
import assert from 'node:assert/strict';

// Dynamic imports so env is set before config.js runs
const { forwardOllalinkToRoom } = await import('../src/server.js');
const { createRoom, joinRoom, leaveRoom } = await import('../src/rooms.js');

function makeFakeWs() {
  const sent = [];
  return { readyState: 1, _sent: sent, send(data, opts) { sent.push({ data, binary: opts?.binary || false }); } };
}

function makeClient(ws, room, sessionId, sourceLang, targetLang) {
  return { ws, session: { sessionId, sourceLang, targetLang }, room, _cleaned: false, _replacedByNewConnection: false };
}

test('baseline: audio forwarded when language matches peer targetLang', () => {
  const room = createRoom();
  const ws1 = makeFakeWs(), ws2 = makeFakeWs();
  joinRoom(room.code, { sessionId: 's1', userId: 'u1', sourceLang: 'en', targetLang: 'hi', displayName: 'A', captionsOn: true, ws: ws1, joinedAt: Date.now() });
  joinRoom(room.code, { sessionId: 's2', userId: 'u2', sourceLang: 'hi', targetLang: 'en', displayName: 'B', captionsOn: true, ws: ws2, joinedAt: Date.now() });
  const c = makeClient(ws1, room, 's1', 'en', 'hi');
  const evt = { kind: 'audio', payload: { pcm: Buffer.from([0, 0]), codec: 'pcm_s16le', sampleRate: 48000, language: 'en', chunkSeq: 1, last: false, utteranceId: 'u1' } };
  forwardOllalinkToRoom(c, evt);
  assert.ok(ws2._sent.length >= 2, `Expected >=2 frames (header+binary), got ${ws2._sent.length}`);
  leaveRoom(room.code, 's1', ws1); leaveRoom(room.code, 's2', ws2);
});

test('FIX-1: audio forwarded when Ollalink omits language field (was silently dropped)', () => {
  const room = createRoom();
  const ws1 = makeFakeWs(), ws2 = makeFakeWs();
  joinRoom(room.code, { sessionId: 's1', userId: 'u1', sourceLang: 'en', targetLang: 'hi', displayName: 'A', captionsOn: true, ws: ws1, joinedAt: Date.now() });
  joinRoom(room.code, { sessionId: 's2', userId: 'u2', sourceLang: 'hi', targetLang: 'en', displayName: 'B', captionsOn: true, ws: ws2, joinedAt: Date.now() });
  const c = makeClient(ws1, room, 's1', 'en', 'hi');
  const evt = { kind: 'audio', payload: { pcm: Buffer.from([0, 0]), codec: 'pcm_s16le', sampleRate: 48000, language: '', chunkSeq: 2, last: false, utteranceId: 'u2' } };
  forwardOllalinkToRoom(c, evt);
  assert.ok(ws2._sent.length >= 2, `Expected >=2 frames, got ${ws2._sent.length} -- FIX-1 REGRESSION: audio silently dropped`);
  leaveRoom(room.code, 's1', ws1); leaveRoom(room.code, 's2', ws2);
});

test('FIX-1: audio forwarded when pBase set but peer has no targetLang (was silently dropped)', () => {
  const room = createRoom();
  const ws1 = makeFakeWs(), ws2 = makeFakeWs();
  joinRoom(room.code, { sessionId: 's1', userId: 'u1', sourceLang: 'en', targetLang: '', displayName: 'A', captionsOn: true, ws: ws1, joinedAt: Date.now() });
  joinRoom(room.code, { sessionId: 's2', userId: 'u2', sourceLang: 'en', targetLang: '', displayName: 'B', captionsOn: true, ws: ws2, joinedAt: Date.now() });
  const c = makeClient(ws1, room, 's1', 'en', '');
  const evt = { kind: 'audio', payload: { pcm: Buffer.from([0, 0]), codec: 'pcm_s16le', sampleRate: 48000, language: 'en', chunkSeq: 3, last: false, utteranceId: 'u3' } };
  forwardOllalinkToRoom(c, evt);
  assert.ok(ws2._sent.length >= 2, `Expected >=2 frames, got ${ws2._sent.length} -- FIX-1 REGRESSION`);
  leaveRoom(room.code, 's1', ws1); leaveRoom(room.code, 's2', ws2);
});

test('correct: wrong-lane audio blocked (hi audio to en-target peer)', () => {
  const room = createRoom();
  const ws1 = makeFakeWs(), ws2 = makeFakeWs();
  joinRoom(room.code, { sessionId: 's1', userId: 'u1', sourceLang: 'en', targetLang: 'fr', displayName: 'A', captionsOn: true, ws: ws1, joinedAt: Date.now() });
  joinRoom(room.code, { sessionId: 's2', userId: 'u2', sourceLang: 'fr', targetLang: 'en', displayName: 'B', captionsOn: true, ws: ws2, joinedAt: Date.now() });
  const c = makeClient(ws1, room, 's1', 'en', 'fr');
  const evt = { kind: 'audio', payload: { pcm: Buffer.from([0, 0]), codec: 'pcm_s16le', sampleRate: 48000, language: 'hi', chunkSeq: 4, last: false, utteranceId: 'u4' } };
  forwardOllalinkToRoom(c, evt);
  assert.equal(ws2._sent.length, 0, `Expected 0 frames (wrong lane), got ${ws2._sent.length}`);
  leaveRoom(room.code, 's1', ws1); leaveRoom(room.code, 's2', ws2);
});

test('end-of-utterance marker: JSON-only, hasBinary=false, endOfUtterance=true', () => {
  const room = createRoom();
  const ws1 = makeFakeWs(), ws2 = makeFakeWs();
  joinRoom(room.code, { sessionId: 's1', userId: 'u1', sourceLang: 'en', targetLang: 'hi', displayName: 'A', captionsOn: true, ws: ws1, joinedAt: Date.now() });
  joinRoom(room.code, { sessionId: 's2', userId: 'u2', sourceLang: 'hi', targetLang: 'en', displayName: 'B', captionsOn: true, ws: ws2, joinedAt: Date.now() });
  const c = makeClient(ws1, room, 's1', 'en', 'hi');
  const evt = { kind: 'audio', payload: { pcm: null, codec: 'pcm_s16le', sampleRate: 48000, language: 'en', chunkSeq: 99, last: true, utteranceId: 'u5' } };
  forwardOllalinkToRoom(c, evt);
  assert.equal(ws2._sent.length, 1, `Expected 1 frame (JSON marker only), got ${ws2._sent.length}`);
  const p = JSON.parse(ws2._sent[0].data);
  assert.equal(p.hasBinary, false);
  assert.equal(p.endOfUtterance, true);
  leaveRoom(room.code, 's1', ws1); leaveRoom(room.code, 's2', ws2);
});
