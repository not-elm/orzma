// NOTE: preload ordering — this script runs AFTER ozma_bridge.js, so window.ozma
// is already defined and frozen; injected before the bridge it would throw on the
// `window.ozma` reads below.
(function () {
  var ozma = window.ozma;
  var ALPHABET = 'sadfjklewcmpgh';
  var OVERLAY_ID = '__ozmaHints';
  var state = null;

  function isVisible(el) {
    var r = el.getBoundingClientRect();
    if (r.width === 0 || r.height === 0) return false;
    if (r.bottom < 0 || r.right < 0 || r.top > window.innerHeight || r.left > window.innerWidth) {
      return false;
    }
    var s = getComputedStyle(el);
    return s.visibility !== 'hidden' && s.display !== 'none' && parseFloat(s.opacity) > 0;
  }

  function classify(el) {
    var tag = el.tagName.toLowerCase();
    if (tag === 'a' && el.hasAttribute('href')) return 'link';
    if (tag === 'textarea' || tag === 'select') return 'input';
    if (tag === 'input') {
      var t = (el.getAttribute('type') || 'text').toLowerCase();
      var clickable = t === 'button' || t === 'submit' || t === 'reset' ||
        t === 'checkbox' || t === 'radio' || t === 'file' || t === 'image';
      return clickable ? 'button' : 'input';
    }
    return 'button';
  }

  function generateLabels(n) {
    var a = ALPHABET, k = a.length, labels = [];
    if (n <= k) {
      for (var i = 0; i < n; i++) labels.push(a[i]);
      return labels;
    }
    for (var i = 0; i < k && labels.length < n; i++) {
      for (var j = 0; j < k && labels.length < n; j++) {
        labels.push(a[i] + a[j]);
      }
    }
    return labels;
  }

  function teardown() {
    var o = document.getElementById(OVERLAY_ID);
    if (o) o.remove();
    state = null;
  }

  function show() {
    teardown();
    var sel = 'a[href], button, input, textarea, select, [role=button], [onclick]';
    var els = Array.prototype.slice.call(document.querySelectorAll(sel)).filter(isVisible);
    if (els.length === 0) {
      ozma.call('hintResult', { kind: 'empty' });
      return;
    }
    var labels = generateLabels(els.length);
    if (els.length > labels.length) {
      els = els.slice(0, labels.length);
    }
    var overlay = document.createElement('div');
    overlay.id = OVERLAY_ID;
    overlay.setAttribute('style', 'position:fixed;left:0;top:0;width:0;height:0;z-index:2147483647;');
    var targets = [];
    for (var i = 0; i < els.length; i++) {
      var el = els[i];
      var label = labels[i];
      var r = el.getBoundingClientRect();
      var badge = document.createElement('div');
      badge.textContent = label.toUpperCase();
      badge.setAttribute('style',
        'position:fixed;left:' + Math.max(0, Math.floor(r.left)) + 'px;' +
        'top:' + Math.max(0, Math.floor(r.top)) + 'px;' +
        'background:#ffd76e;color:#302505;font:bold 11px/14px monospace;' +
        'padding:0 3px;border-radius:3px;box-shadow:0 1px 2px rgba(0,0,0,.4);');
      overlay.appendChild(badge);
      targets.push({ el: el, label: label, kind: classify(el), badge: badge });
    }
    document.documentElement.appendChild(overlay);
    state = { targets: targets, prefix: '' };
  }

  function activate(t) {
    teardown();
    if (t.kind === 'input') {
      t.el.focus();
      ozma.call('hintResult', { kind: 'focusedInput' });
    } else {
      t.el.click();
      ozma.call('hintResult', { kind: t.kind === 'link' ? 'navigated' : 'clicked' });
    }
  }

  function refilter() {
    var p = state.prefix;
    var match = null;
    var remaining = 0;
    for (var i = 0; i < state.targets.length; i++) {
      var t = state.targets[i];
      var hit = t.label.indexOf(p) === 0;
      t.badge.style.display = hit ? '' : 'none';
      if (hit) {
        remaining++;
        if (t.label === p) match = t;
      }
    }
    if (match && remaining === 1) activate(match);
  }

  ozma.on('hints:show', function () { show(); });
  ozma.on('hints:hide', function () { teardown(); });
  ozma.on('hints:key', function (payload) {
    if (!state) return;
    if (payload && payload.backspace) {
      if (state.prefix.length === 0) return;
      state.prefix = state.prefix.slice(0, -1);
    } else {
      var key = payload && payload.key;
      if (!key) return;
      var ch = key.toLowerCase();
      if (ALPHABET.indexOf(ch) === -1) return;
      var next = state.prefix + ch;
      var any = state.targets.some(function (t) { return t.label.indexOf(next) === 0; });
      if (!any) return;
      state.prefix = next;
    }
    refilter();
  });
})();
