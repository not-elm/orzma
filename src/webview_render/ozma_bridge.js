// NOTE: bevy_cef contract — Rust->JS cef.listen delivers a JSON *string* (hence
// JSON.parse); JS->Rust cef.emit serializes only its FIRST argument. Replies
// arrive on the "ozma" channel keyed by reqId; push events on "ozma.event".
// The {__u8} base64 tag carries a Uint8Array, but only as a top-level value (the
// `params` sent, or a reply `value`/event `payload` received via decodeValue) —
// bytes nested inside an object/array param are NOT tagged and won't round-trip.
// window.ozma is frozen so a page cannot shadow it.
(function () {
  var cef = window.cef;
  var nextId = 0;
  // NOTE: a call that never receives a reply leaks its calls entry and pending Promise; the Rust side MUST send an error reply (e.g. on owner disconnect) to clear it. There is no client-side timeout.
  var calls = new Map();
  var listeners = new Map();

  function encodeParam(p) {
    if (p instanceof Uint8Array) {
      var bin = '';
      for (var i = 0; i < p.length; i++) bin += String.fromCharCode(p[i]);
      return { __u8: btoa(bin) };
    }
    return p;
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

  cef.listen('ozma', function (raw) {
    var frame = typeof raw === 'string' ? JSON.parse(raw) : raw;
    var c = calls.get(frame.reqId);
    if (!c) return;
    calls.delete(frame.reqId);
    if (frame.ok) c.resolve(decodeValue(frame.value));
    else c.reject(new Error(frame.error));
  });

  cef.listen('ozma.event', function (raw) {
    var frame = typeof raw === 'string' ? JSON.parse(raw) : raw;
    var hs = listeners.get(frame.event);
    if (!hs) return;
    var payload = decodeValue(frame.payload);
    hs.slice().forEach(function (h) { try { h(payload); } catch (e) {} });
  });

  var api = {
    call: function (method, params) {
      var reqId = 'o' + nextId++;
      var encoded = encodeParam(params);
      return new Promise(function (resolve, reject) {
        calls.set(reqId, { resolve: resolve, reject: reject });
        try {
          cef.emit({ kind: 'ozma.call', reqId: reqId, method: method, params: encoded });
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

  Object.defineProperty(window, 'ozma', { value: Object.freeze(api), configurable: false, writable: false });

  // NOTE: length > 1 means a page-specific handler is also registered (e.g. ozmd's);
  // in that case this default bows out to avoid double-scrolling. For remote pages with
  // no custom handler, this is the sole handler (length === 1) and it performs the scroll.
  api.on('scroll', function (payload) {
    if ((listeners.get('scroll') || []).length > 1) { return; }
    var action = payload && payload.action;
    var line = 60;
    var page = window.innerHeight;
    var max = Math.max(0, document.documentElement.scrollHeight - window.innerHeight);
    switch (action) {
      case 'down':     window.scrollBy({ top: line });     break;
      case 'up':       window.scrollBy({ top: -line });    break;
      case 'halfDown': window.scrollBy({ top: page / 2 }); break;
      case 'halfUp':   window.scrollBy({ top: -page / 2 }); break;
      case 'pageDown': window.scrollBy({ top: page });     break;
      case 'pageUp':   window.scrollBy({ top: -page });    break;
      case 'top':      window.scrollTo({ top: 0 });        break;
      case 'bottom':   window.scrollTo({ top: max });      break;
    }
  });
})();
