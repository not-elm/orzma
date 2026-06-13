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
        cef.emit({ kind: 'ozmux.call', reqId: reqId, method: method, args: encoded });
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

  Object.defineProperty(window, 'ozmux', { value: Object.freeze(api), configurable: false, writable: false });
})();
