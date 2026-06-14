// NOTE: bevy_cef contract — Rust->JS cef.listen delivers a JSON *string* (hence
// JSON.parse); JS->Rust cef.emit serializes only its FIRST argument. Replies
// arrive on the "ozmux" channel keyed by reqId; push events on "ozmux.event".
// The {__u8} base64 tag mirrors binary-codec.ts (top-level only). window.ozmux
// is frozen so a page cannot shadow it.
(function () {
  var cef = window.cef;
  var nextId = 0;
  // NOTE: a call that never receives a reply leaks its calls entry and pending Promise; the Rust side MUST send an error reply (e.g. on owner disconnect) to clear it. There is no client-side timeout.
  var calls = new Map();
  var listeners = new Map();

  function encodeArg(a) {
    if (a instanceof Uint8Array) {
      var bin = '';
      for (var i = 0; i < a.length; i++) bin += String.fromCharCode(a[i]);
      return { __u8: btoa(bin) };
    }
    return a;
  }
  function decodeValue(v) {
    if (v && typeof v === 'object' && typeof v.__u8 === 'string') {
      var s = atob(v.__u8);
      var out = new Uint8Array(s.length);
      for (var i = 0; i < s.length; i++) out[i] = s.charCodeAt(i);
      return out;
    }
    return v;
  }

  cef.listen('ozmux', function (raw) {
    var frame = typeof raw === 'string' ? JSON.parse(raw) : raw;
    var c = calls.get(frame.reqId);
    if (!c) return;
    calls.delete(frame.reqId);
    if (frame.ok) c.resolve(decodeValue(frame.value));
    else c.reject(new Error(frame.error));
  });

  cef.listen('ozmux.event', function (raw) {
    var frame = typeof raw === 'string' ? JSON.parse(raw) : raw;
    var hs = listeners.get(frame.event);
    if (!hs) return;
    var payload = decodeValue(frame.payload);
    hs.slice().forEach(function (h) { try { h(payload); } catch (e) {} });
  });

  var api = {
    call: function (method, args) {
      var reqId = 'o' + nextId++;
      var encoded = (args || []).map(encodeArg);
      return new Promise(function (resolve, reject) {
        calls.set(reqId, { resolve: resolve, reject: reject });
        try {
          cef.emit({ kind: 'ozmux.call', reqId: reqId, method: method, args: encoded });
        } catch (e) {
          // A synchronous emit failure never gets a reply, so settle and drop the
          // entry here — otherwise it leaks in calls forever.
          calls.delete(reqId);
          reject(e);
        }
      });
    },
    on: function (event, handler) {
      var hs = listeners.get(event) || [];
      hs.push(handler);
      listeners.set(event, hs);
    },
    off: function (event, handler) {
      var hs = listeners.get(event);
      if (hs) listeners.set(event, hs.filter(function (h) { return h !== handler; }));
    },
  };

  // NOTE: only Alt-modified keys are intercepted, so IME composition keys
  // (bare keys, keyCode 229) are never swallowed.
  var handle = location.hostname;
  var navMap = { h: 'left', j: 'down', k: 'up', l: 'right' };
  var keymap = { mods: ['alt'], keys: navMap };
  function matchNav(e) {
    if (!keymap.mods.every(function (m) {
      return m === 'alt' ? e.altKey : m === 'ctrl' ? e.ctrlKey : m === 'shift' ? e.shiftKey : m === 'meta' ? e.metaKey : false;
    })) return null;
    var k = (e.key || '').toLowerCase();
    return Object.prototype.hasOwnProperty.call(keymap.keys, k) ? keymap.keys[k] : null;
  }
  window.addEventListener('keydown', function (e) {
    var dir = matchNav(e);
    if (dir) {
      e.preventDefault();
      e.stopPropagation();
      api.call('__ozma.nav', [handle, dir]);
    }
  }, true);
  window.addEventListener('focus', function () { api.call('__ozma.focus', [handle, true]); });
  window.addEventListener('blur', function () { api.call('__ozma.focus', [handle, false]); });
  api.on('__ozma.keys', function (set) {
    if (set && set.keys) keymap = { mods: set.mods || ['alt'], keys: set.keys };
  });

  Object.defineProperty(window, 'ozmux', { value: Object.freeze(api), configurable: false, writable: false });
})();
