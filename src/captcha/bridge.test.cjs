const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const assert = require('node:assert/strict');
const source = fs.readFileSync(path.join(__dirname, 'bridge.js'), 'utf8');
function setup() {
  const sent = [], listeners = {};
  const window = {ipc: {postMessage: s => sent.push(JSON.parse(s))}, addEventListener: (key, fn) => listeners[key] = fn};
  const context = vm.createContext({window, URL, location: {href: 'https://ssl.captcha.qq.com/template/wireless_mqq_captcha.html'}});
  vm.runInContext(source, context);
  return {context, sent, listeners};
}
{
  const {context, sent} = setup();
  vm.runInContext(`window.calls = 0;
    window.TencentCaptcha = function(id, callback) { this.callback = callback; };
    const captcha = new window.TencentCaptcha('id', () => window.calls++);
    captcha.callback({ret: 2});
    captcha.callback({ret: 0, ticket: 't', randstr: 'r'});
    captcha.callback({ret: 0, ticket: 'duplicate', randstr: 'r'});`, context);
  assert.equal(sent.length, 1);
  assert.equal(sent[0].ticket, 't');
  assert.equal(context.window.calls, 3);
}
{
  const {context, sent} = setup();
  vm.runInContext(`window.calls = 0; window.mqq = {invoke: () => window.calls++};
    window.mqq.invoke('OTHER', 'onVerifyCAPTCHA', {result: '0', ticket: 'x', randstr: 'y'});
    window.mqq.invoke('CAPTCHA', 'onVerifyCAPTCHA', {result: '1'});
    window.mqq.invoke('CAPTCHA', 'onVerifyCAPTCHA', {result: '0', ticket: 't', randstr: 'r'});`, context);
  assert.equal(sent.length, 1);
  assert.equal(context.window.calls, 3);
}
{
  const {sent, listeners} = setup();
  listeners.message({origin: 'https://evil.example', data: {ret: 0, ticket: 'evil', randstr: 'x'}});
  assert.equal(sent.length, 0);
  listeners.message({origin: 'https://turing.captcha.qcloud.com', data: JSON.stringify({data: {ret: 0, ticket: 't', randstr: 'r'}})});
  assert.equal(sent.length, 1);
}
console.log('Captcha bridge: SDK and QQ H5 callbacks, origin checks, duplicate suppression passed');
