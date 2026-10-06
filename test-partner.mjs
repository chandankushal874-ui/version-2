#!/usr/bin/env node
/**
 * test-partner.mjs â€” Solo PC Live 1:1 Call Partner Simulator
 *
 * Designed for solo testing on a single PC without needing a second person.
 * Connects to your active room code, acts as your bilingual call partner (e.g. Hindi),
 * listens to your voice, displays translations, and speaks back in native voice so you
 * can test end-to-end voice translation through your speakers!
 *
 * Usage:
 *   node test-partner.mjs [ROOM_CODE] [RELAY_URL]
 *   Example: node test-partner.mjs ABC123
 */

import readline from 'readline';

const API_KEY = process.env.OLLALINK_API_KEY || process.env.OLLALINK_DASHBOARD_KEY || 'sk_44935c9a9c2186a08697dd56ddc734cb165b94b060aebfd4';
const DEFAULT_RELAY = 'https://windows-live-translation-app-lzx2.onrender.com';

const args = process.argv.slice(2);
let roomCode = (args[0] || '').trim().toUpperCase();
const relayBase = (args[1] || DEFAULT_RELAY).trim().replace(/\/+$/, '');

const rl = readline.createInterface({
  input: process.stdin,
  output: process.stdout,
});

function prompt(query) {
  return new Promise((resolve) => rl.question(query, resolve));
}

// ANSI Color formatting
const C = {
  reset: '\x1b[0m',
  bold: '\x1b[1m',
  green: '\x1b[32m',
  cyan: '\x1b[36m',
  yellow: '\x1b[33m',
  magenta: '\x1b[35m',
  red: '\x1b[31m',
  gray: '\x1b[90m',
};

console.log(`
${C.bold}${C.cyan}========================================================================${C.reset}
${C.bold}${C.cyan}      Ollalink Translate â€” Solo PC Interactive Test Partner Bot          ${C.reset}
${C.bold}${C.cyan}========================================================================${C.reset}
${C.gray}Relay Server:${C.reset} ${relayBase}
${C.gray}Partner Identity:${C.reset} Pooja (Speaks: Hindi âž” Hears: English)
`);

if (!roomCode) {
  roomCode = (await prompt(`${C.bold}${C.yellow}Enter the 6-character room code from your desktop app: ${C.reset}`)).trim().toUpperCase();
}

if (!roomCode || roomCode.length < 4) {
  console.log(`${C.red}Error: Invalid room code provided. Exiting.${C.reset}`);
  process.exit(1);
}

console.log(`\n${C.cyan}1. Minting session token for partner bot...${C.reset}`);
let sessionToken = '';
let wsEndpoint = '';
try {
  const sessionRes = await fetch(`${relayBase}/api/session`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({
      userId: 'solo-test-partner-' + Math.floor(Math.random() * 10000),
      displayName: 'Pooja (Hindi Partner)',
      sourceLang: 'hi',
      targetLang: 'en',
      voice: 'nh-m01',
    }),
  });

  const sessionData = await sessionRes.json();
  if (!sessionData.token) {
    throw new Error(sessionData.error || 'Failed to mint session token');
  }
  sessionToken = sessionData.token;
  wsEndpoint = sessionData.wsUrl || `${relayBase.replace(/^http/i, 'ws')}/call`;
  console.log(`${C.green}âœ“ Session minted successfully (Session ID: ${sessionData.sessionId?.slice(0, 8)}...)${C.reset}`);
} catch (err) {
  console.error(`${C.red}Failed to connect to relay server:${C.reset}`, err.message);
  process.exit(1);
}

console.log(`${C.cyan}2. Connecting to room [${C.bold}${roomCode}${C.cyan}]...${C.reset}`);
const ws = new WebSocket(wsEndpoint);

let isJoined = false;
let autoReply = true;
let isSpeaking = false;

ws.onopen = () => {
  ws.send(JSON.stringify({
    type: 'join',
    token: sessionToken,
    room: roomCode,
    sourceLang: 'hi',
    targetLang: 'en',
    displayName: 'Pooja (Hindi Partner)',
    captionsOn: true,
  }));
};

// 48kHz mono PCM to 16kHz mono PCM decimation (every 3rd sample)
function downsample48to16(buf48) {
  const inSamples = Math.floor(buf48.length / 2);
  const outSamples = Math.floor(inSamples / 3);
  const outBuf = Buffer.alloc(outSamples * 2);
  for (let i = 0; i < outSamples; i++) {
    const s = buf48.readInt16LE(i * 3 * 2);
    outBuf.writeInt16LE(s, i * 2);
  }
  return outBuf;
}

// Synthesizes Hindi text to audio and streams 16kHz PCM to relay
async function speakPhrase(textHindi, description) {
  if (isSpeaking) {
    console.log(`${C.yellow}(Bot is already speaking, please wait...)${C.reset}`);
    return;
  }
  isSpeaking = true;
  console.log(`\n${C.magenta}ðŸŽ™ï¸ Pooja is speaking: "${textHindi}"${C.reset} ${C.gray}(${description})${C.reset}`);

  try {
    const ttsWs = new WebSocket('wss://sound-stream.ollalink.com/v1/tts/speak', {
      headers: { 'X-NH-GPU-Key': API_KEY },
    });

    const audioBuffers48 = [];
    await new Promise((resolve, reject) => {
      const timeout = setTimeout(() => {
        try { ttsWs.close(); } catch {}
        reject(new Error('TTS timeout'));
      }, 10000);

      ttsWs.onopen = () => {
        ttsWs.send(JSON.stringify({
          type: 'session.configure',
          api_key: API_KEY,
          voice: 'nh-m01',
          language: 'hi',
        }));
      };

      ttsWs.onmessage = async (e) => {
        if (typeof e.data === 'string') {
          try {
            const msg = JSON.parse(e.data);
            if (msg.type === 'session.ready') {
              ttsWs.send(JSON.stringify({ type: 'speak', text: textHindi }));
            } else if (msg.type === 'speak.done') {
              clearTimeout(timeout);
              try { ttsWs.close(); } catch {}
              resolve();
            } else if (msg.type === 'error') {
              clearTimeout(timeout);
              try { ttsWs.close(); } catch {}
              reject(new Error(msg.detail || msg.message));
            }
          } catch {}
        } else {
          const buf = Buffer.from(await e.data.arrayBuffer());
          audioBuffers48.push(buf);
        }
      };

      ttsWs.onerror = (e) => {
        clearTimeout(timeout);
        reject(new Error('TTS WebSocket connection failed'));
      };
    });

    const full48 = Buffer.concat(audioBuffers48);
    const full16 = downsample48to16(full48);

    // Stream 16kHz PCM chunks at real-time (500ms intervals = 16,000 bytes per chunk)
    const chunkSize = 16000;
    for (let offset = 0; offset < full16.length; offset += chunkSize) {
      if (ws.readyState !== 1) break;
      const chunk = full16.subarray(offset, Math.min(offset + chunkSize, full16.length));
      ws.send(chunk);
      await new Promise((r) => setTimeout(r, 480));
    }

    console.log(`${C.green}âœ“ Speech streamed to call. Check your desktop app speaker!${C.reset}\n`);
  } catch (err) {
    console.error(`${C.red}TTS Synthesis failed:${C.reset}`, err.message);
  } finally {
    isSpeaking = false;
  }
}

ws.onmessage = async (e) => {
  if (typeof e.data !== 'string') return;
  let msg;
  try { msg = JSON.parse(e.data); } catch { return; }

  switch (msg.type) {
    case 'joined': {
      isJoined = true;
      console.log(`\n${C.bold}${C.green}========================================================================${C.reset}`);
      console.log(`${C.bold}${C.green}  ðŸŽ‰ Successfully joined call room ${roomCode}!${C.reset}`);
      console.log(`${C.bold}${C.green}  Your desktop app should now show Pooja connected!${C.reset}`);
      console.log(`${C.bold}${C.green}========================================================================${C.reset}\n`);

      showMenu();

      // Automatically speak greeting after 1.5 seconds
      setTimeout(() => {
        speakPhrase(
          'à¤¨à¤®à¤¸à¥à¤¤à¥‡! à¤®à¥ˆà¤‚ à¤†à¤ªà¤•à¥€ à¤Ÿà¥‡à¤¸à¥à¤Ÿ à¤ªà¤¾à¤°à¥à¤Ÿà¤¨à¤° à¤ªà¥‚à¤œà¤¾ à¤¹à¥‚à¤à¥¤ à¤•à¥à¤¯à¤¾ à¤†à¤ªà¤•à¥‹ à¤®à¥‡à¤°à¥€ à¤†à¤µà¤¾à¤œà¤¼ à¤¸à¤¾à¤«à¤¼ à¤¸à¥à¤¨à¤¾à¤ˆ à¤¦à¥‡ à¤°à¤¹à¥€ à¤¹à¥ˆ?',
          'Greeting: "Hello! I am your test partner Pooja. Can you hear my voice clearly?"'
        );
      }, 1500);
      break;
    }

    case 'peer-joined': {
      console.log(`\n${C.cyan}[Peer Connected]: ${msg.peer?.displayName || 'Partner'}${C.reset}`);
      break;
    }

    case 'peer-left': {
      console.log(`\n${C.yellow}[Peer Left]: ${msg.sessionId}${C.reset}`);
      break;
    }

    case 'caption': {
      const payload = msg.payload;
      if (!payload) break;

      if (msg.kind === 'caption-partial') {
        process.stdout.write(`\r${C.gray}[Speaking...]: ${payload.text || ''}${C.reset}`);
      } else if (msg.kind === 'caption-final') {
        console.log(`\n${C.bold}${C.green}ðŸ—£ï¸  [You Said (English)]:${C.reset} "${payload.text}"`);
      } else if (msg.kind === 'translation') {
        const transText = payload.text || '';
        console.log(`${C.bold}${C.yellow}ðŸŒ [Translated to Hindi]:${C.reset} "${transText}"\n`);

        // If auto-reply is on, partner automatically responds in Hindi
        if (autoReply && !isSpeaking && transText.trim()) {
          setTimeout(() => {
            const replies = [
              'à¤¹à¤¾à¤, à¤®à¥à¤à¥‡ à¤†à¤ªà¤•à¥€ à¤¬à¤¾à¤¤ à¤¸à¤®à¤ à¤† à¤—à¤ˆ! à¤†à¤µà¤¾à¤œà¤¼ à¤¬à¤¿à¤²à¥à¤•à¥à¤² à¤¸à¤¾à¤«à¤¼ à¤”à¤° à¤¸à¥à¤ªà¤·à¥à¤Ÿ à¤† à¤°à¤¹à¥€ à¤¹à¥ˆà¥¤',
              'à¤¬à¤¹à¥à¤¤ à¤¬à¤¢à¤¼à¤¿à¤¯à¤¾! à¤¯à¤¹ à¤²à¤¾à¤‡à¤µ à¤µà¥‰à¤¯à¤¸ à¤Ÿà¥à¤°à¤¾à¤‚à¤¸à¤²à¥‡à¤¶à¤¨ à¤¬à¤¹à¥à¤¤ à¤¤à¥‡à¤œà¤¼ à¤”à¤° à¤¸à¤Ÿà¥€à¤• à¤¹à¥ˆà¥¤',
              'à¤¹à¤¾à¤ à¤¬à¤¿à¤²à¥à¤•à¥à¤², à¤®à¥ˆà¤‚ à¤†à¤ªà¤•à¥‹ à¤¸à¥à¤¨ à¤ªà¤¾ à¤°à¤¹à¥€ à¤¹à¥‚à¤à¥¤ à¤•à¥à¤¯à¤¾ à¤†à¤ª à¤®à¥à¤à¥‡ à¤¸à¥à¤¨ à¤¸à¤•à¤¤à¥‡ à¤¹à¥ˆà¤‚?',
            ];
            const chosen = replies[Math.floor(Math.random() * replies.length)];
            speakPhrase(chosen, 'Auto-Reply');
          }, 1800);
        }
      }
      break;
    }

    case 'error': {
      console.error(`\n${C.red}[Relay Error]:${C.reset}`, msg.message || msg.code);
      break;
    }
  }
};

ws.onerror = (err) => {
  console.error(`\n${C.red}WebSocket Error:${C.reset}`, err.message || err);
};

ws.onclose = () => {
  console.log(`\n${C.yellow}Disconnected from call room.${C.reset}`);
  process.exit(0);
};

function showMenu() {
  console.log(`${C.bold}--- Interactive Test Controls ---${C.reset}`);
  console.log(`  ${C.cyan}[1]${C.reset} Speak Greeting:      "à¤¨à¤®à¤¸à¥à¤¤à¥‡! à¤•à¥à¤¯à¤¾ à¤†à¤ªà¤•à¥‹ à¤®à¥‡à¤°à¥€ à¤†à¤µà¤¾à¤œà¤¼ à¤† à¤°à¤¹à¥€ à¤¹à¥ˆ?"`);
  console.log(`  ${C.cyan}[2]${C.reset} Speak Status:        "à¤…à¤¨à¥à¤µà¤¾à¤¦ à¤¬à¤¹à¥à¤¤ à¤¤à¥‡à¤œà¤¼ à¤”à¤° à¤¸à¥à¤šà¤¾à¤°à¥‚ à¤°à¥‚à¤ª à¤¸à¥‡ à¤•à¤¾à¤® à¤•à¤° à¤°à¤¹à¤¾ à¤¹à¥ˆà¥¤"`);
  console.log(`  ${C.cyan}[3]${C.reset} Speak Question:      "à¤†à¤œ à¤†à¤ªà¤•à¤¾ à¤¦à¤¿à¤¨ à¤•à¥ˆà¤¸à¤¾ à¤¬à¥€à¤¤ à¤°à¤¹à¤¾ à¤¹à¥ˆ?"`);
  console.log(`  ${C.cyan}[4]${C.reset} Speak Farewell:      "à¤†à¤ªà¤¸à¥‡ à¤¬à¤¾à¤¤ à¤•à¤°à¤•à¥‡ à¤¬à¤¹à¥à¤¤ à¤…à¤šà¥à¤›à¤¾ à¤²à¤—à¤¾, à¤…à¤²à¤µà¤¿à¤¦à¤¾!"`);
  console.log(`  ${C.cyan}[a]${C.reset} Toggle Auto-Reply:   (Currently: ${autoReply ? C.green + 'ON' : C.red + 'OFF'}${C.reset})`);
  console.log(`  ${C.cyan}[q]${C.reset} Leave and Quit`);
  console.log(`${C.gray}Or type any custom phrase in Hindi or English and press Enter to speak it!${C.reset}\n`);
}

rl.on('line', async (line) => {
  const input = line.trim();
  if (!input) return;

  if (input === '1') {
    await speakPhrase(
      'à¤¨à¤®à¤¸à¥à¤¤à¥‡! à¤•à¥à¤¯à¤¾ à¤†à¤ªà¤•à¥‹ à¤®à¥‡à¤°à¥€ à¤†à¤µà¤¾à¤œà¤¼ à¤¸à¤¾à¤«à¤¼ à¤† à¤°à¤¹à¥€ à¤¹à¥ˆ?',
      '"Hello! Can you hear my voice clearly?"'
    );
  } else if (input === '2') {
    await speakPhrase(
      'à¤…à¤¨à¥à¤µà¤¾à¤¦ à¤¬à¤¹à¥à¤¤ à¤¤à¥‡à¤œà¤¼ à¤”à¤° à¤¸à¥à¤šà¤¾à¤°à¥‚ à¤°à¥‚à¤ª à¤¸à¥‡ à¤•à¤¾à¤® à¤•à¤° à¤°à¤¹à¤¾ à¤¹à¥ˆà¥¤',
      '"Translation is working very fast and smoothly."'
    );
  } else if (input === '3') {
    await speakPhrase(
      'à¤†à¤œ à¤†à¤ªà¤•à¤¾ à¤¦à¤¿à¤¨ à¤•à¥ˆà¤¸à¤¾ à¤¬à¥€à¤¤ à¤°à¤¹à¤¾ à¤¹à¥ˆ?',
      '"How is your day going today?"'
    );
  } else if (input === '4') {
    await speakPhrase(
      'à¤†à¤ªà¤¸à¥‡ à¤¬à¤¾à¤¤ à¤•à¤°à¤•à¥‡ à¤¬à¤¹à¥à¤¤ à¤…à¤šà¥à¤›à¤¾ à¤²à¤—à¤¾, à¤…à¤²à¤µà¤¿à¤¦à¤¾!',
      '"It was wonderful talking with you, goodbye!"'
    );
  } else if (input.toLowerCase() === 'a') {
    autoReply = !autoReply;
    console.log(`\n${C.yellow}Auto-Reply is now ${autoReply ? C.green + 'ENABLED' : C.red + 'DISABLED'}${C.reset}\n`);
  } else if (input.toLowerCase() === 'q') {
    console.log('Leaving room...');
    ws.close();
    process.exit(0);
  } else {
    // Custom message
    await speakPhrase(input, `Custom phrase: "${input}"`);
  }
});

