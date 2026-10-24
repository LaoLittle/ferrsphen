(() => {
  const allowed = value => {
    try {
      const url = new URL(value);
      return url.protocol === 'https:' &&
        (url.hostname === 'qq.com' || url.hostname.endsWith('.qq.com') ||
         url.hostname === 'captcha.qcloud.com' || url.hostname.endsWith('.captcha.qcloud.com'));
    } catch (_) { return false; }
  };
  if (!allowed(location.href)) return;
  let sent = false;
  const capture = (value, depth = 0) => {
    if (sent || depth > 4) return;
    if (typeof value === 'string') {
      if (value.length > 32768) return;
      try { value = JSON.parse(value); } catch (_) { return; }
    }
    if (!value || typeof value !== 'object') return;
    if (value.ret === 0 && typeof value.ticket === 'string' && value.ticket.length > 0 &&
        value.ticket.length <= 16384 && typeof value.randstr === 'string' &&
        value.randstr.length > 0 && value.randstr.length <= 1024) {
      sent = true;
      window.ipc.postMessage(JSON.stringify({ kind: 'captcha', ret: 0, ticket: value.ticket, randstr: value.randstr }));
      return;
    }
    for (const key of ['data', 'result', 'params']) capture(value[key], depth + 1);
  };
  window.addEventListener('message', event => {
    if (allowed(event.origin)) capture(event.data);
  });
  const wrappedBridges = new WeakMap();
  const wrapBridge = bridge => {
    if (!bridge || typeof bridge !== 'object') return bridge;
    if (wrappedBridges.has(bridge)) return wrappedBridges.get(bridge);
    const proxy = new Proxy(bridge, {
      get(target, key, receiver) {
        const original = Reflect.get(target, key, receiver);
        if (key !== 'invoke' || typeof original !== 'function') return original;
        return function(namespace, method, params) {
          if (namespace === 'CAPTCHA' && method === 'onVerifyCAPTCHA' && params &&
              (params.result === '0' || params.result === 0)) {
            capture({ ret: 0, ticket: params.ticket, randstr: params.randstr });
          }
          return original.apply(target, arguments);
        };
      }
    });
    wrappedBridges.set(bridge, proxy);
    wrappedBridges.set(proxy, proxy);
    return proxy;
  };
  let bridge = wrapBridge(window.mqq);
  Object.defineProperty(window, 'mqq', {
    configurable: true, enumerable: true,
    get: () => bridge,
    set: value => { bridge = wrapBridge(value); }
  });
  const wrap = Constructor => typeof Constructor !== 'function' ? Constructor : new Proxy(Constructor, {
    construct(target, args, newTarget) {
      if (typeof args[1] === 'function') {
        const original = args[1];
        args[1] = function(result) { capture(result); return original.apply(this, arguments); };
      }
      return Reflect.construct(target, args, newTarget);
    },
    apply(target, receiver, args) {
      if (typeof args[1] === 'function') {
        const original = args[1];
        args[1] = function(result) { capture(result); return original.apply(this, arguments); };
      }
      return Reflect.apply(target, receiver, args);
    }
  });
  let constructor = wrap(window.TencentCaptcha);
  Object.defineProperty(window, 'TencentCaptcha', {
    configurable: true, enumerable: true,
    get: () => constructor,
    set: value => { constructor = wrap(value); }
  });
})();
