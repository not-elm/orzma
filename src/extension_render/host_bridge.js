// NOTE: bevy_cef contract — Rust->JS cef.listen delivers a JSON *string* (hence
// JSON.parse); JS->Rust cef.emit serializes only its FIRST argument into one
// global Receive<OzmuxFrame> (single self-describing object, no channel arg, a
// second argument is dropped). The {__u8} base64 tag mirrors binary-codec.ts.
(function () {
  var cef = window.cef;
  var nextId = 0;
  var calls = new Map();

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

  function hostCall(ns, method, args) {
    var reqId = 'h' + nextId++;
    var encoded = args.map(encodeArg);
    return new Promise(function (resolve, reject) {
      calls.set(reqId, { resolve: resolve, reject: reject });
      cef.emit({ kind: 'host.call', reqId: reqId, ns: ns, method: method, args: encoded });
    });
  }

  var granted = window.__ozmuxGranted || [];
  for (var g = 0; g < granted.length; g++) {
    (function (ns) {
      window[ns] = new Proxy(
        {},
        {
          get: function (_t, method) {
            // NOTE: a Symbol key (e.g. Symbol.toPrimitive, or `then` probing for
            // a thenable) must NOT return a callable, or window[ns] would look
            // like a Promise and break. Only string method names dispatch.
            if (typeof method !== 'string') return undefined;
            return function () {
              return hostCall(ns, method, Array.prototype.slice.call(arguments));
            };
          },
        },
      );
    })(granted[g]);
  }
})();
