const WebSocket = require('ws');

const ws = new WebSocket('ws://localhost:18790');

ws.on('open', function open() {
  console.log('connected');
});

ws.on('message', function message(data) {
  console.log('received: %s', data);
  const msg = JSON.parse(data);
  if (msg.type === 'event' && msg.event === 'connect.challenge') {
    console.log('Sending connect request...');
    ws.send(JSON.stringify({
      type: 'req',
      id: '1',
      method: 'connect',
      params: {
        minProtocol: 3,
        maxProtocol: 3,
        client: {
          id: 'test-client',
          version: '0.1.0',
          platform: 'node',
          mode: 'cli'
        },
        caps: [],
        role: 'operator',
        scopes: [],
        userAgent: 'test-client/0.1.0'
      }
    }));
  } else if (msg.type === 'hello-ok') {
    console.log('Handshake successful!');
    process.exit(0);
  }
});

ws.on('close', function close() {
  console.log('disconnected');
});

ws.on('error', console.error);
