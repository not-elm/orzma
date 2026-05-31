// NOTE: bevy_cef contract — Rust->JS cef.listen delivers a JSON *string* (hence
// JSON.parse below); JS->Rust cef.emit serializes only its FIRST argument into
// one global Receive<OzmuxFrame> (hence single-object emit, no channel-name arg —
// a second argument is silently dropped). Mirrors ozmux-bridge.ts (installOzmux).
(function () {
  var cef = window.cef;
  var nextId = 0;
  var calls = new Map();
  var subs = new Map();

  cef.listen('ozmux', function (raw) {
    var frame = typeof raw === 'string' ? JSON.parse(raw) : raw;
    var id = frame.id;
    switch (frame.kind) {
      case 'result': {
        var c = calls.get(id);
        if (c) {
          c.resolve(frame.payload);
          calls.delete(id);
        }
        break;
      }
      case 'error': {
        var ce = calls.get(id);
        if (ce) {
          ce.reject(new Error(frame.code + ': ' + frame.message));
          calls.delete(id);
        }
        break;
      }
      case 'sub.data':
        pushSub(subs.get(id), frame.payload);
        break;
      case 'sub.complete':
        endSub(id);
        break;
      case 'sub.error':
        endSub(id, new Error(frame.code + ': ' + frame.message));
        break;
    }
  });

  function pushSub(s, payload) {
    if (!s) return;
    if (s.waiter) {
      var w = s.waiter;
      s.waiter = undefined;
      w({ value: payload, done: false });
    } else {
      s.queue.push(payload);
    }
  }

  function endSub(id, err) {
    var s = subs.get(id);
    if (!s) return;
    s.done = true;
    if (err) s.error = err;
    if (s.waiter) {
      var w = s.waiter;
      s.waiter = undefined;
      w({ value: undefined, done: true });
    }
    subs.delete(id);
  }

  window.ozmux = {
    call: function (name, payload) {
      var id = 'c' + nextId++;
      return new Promise(function (resolve, reject) {
        calls.set(id, { resolve: resolve, reject: reject });
        cef.emit({ kind: 'call', id: id, name: name, payload: payload });
      });
    },
    subscribe: function (name, params, opts) {
      var id = 's' + nextId++;
      var state = { queue: [], done: false };
      subs.set(id, state);
      cef.emit({ kind: 'sub.open', id: id, name: name, params: params });
      if (opts && opts.signal) {
        opts.signal.addEventListener('abort', function () {
          cef.emit({ kind: 'sub.cancel', id: id });
          endSub(id);
        });
      }
      return {
        // NOTE: single-waiter async iterator — callers must await each next()
        // before calling again. for-await is the only safe usage; concurrent
        // next() calls would overwrite the pending waiter and drop a result.
        [Symbol.asyncIterator]: function () {
          return {
            next: function () {
              if (state.queue.length)
                return Promise.resolve({ value: state.queue.shift(), done: false });
              if (state.error) return Promise.reject(state.error);
              if (state.done) return Promise.resolve({ value: undefined, done: true });
              return new Promise(function (res) {
                state.waiter = res;
              });
            },
          };
        },
      };
    },
  };
})();
