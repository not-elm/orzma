// NOTE: preload ordering — this script runs AFTER ozma_bridge.js, so window.ozma
// is already defined and frozen; injected before the bridge it would throw on the
// `window.ozma` reads below.
(function () {
  var ozma = window.ozma;
  var ALPHABET = 'sadfjklewcmpgh';
  var OVERLAY_ID = '__ozmaHints';
  var state = null;

  function visibleRect(el) {
    if (el.matches && el.matches(':disabled')) return null;
    var r = el.getBoundingClientRect();
    if (r.width === 0 || r.height === 0) return null;
    if (r.bottom < 0 || r.right < 0 || r.top > window.innerHeight || r.left > window.innerWidth) {
      return null;
    }
    if (el.checkVisibility) {
      var ok = el.checkVisibility({
        opacityProperty: true,
        visibilityProperty: true,
        contentVisibilityAuto: true,
      });
      if (!ok) return null;
    } else {
      var s = getComputedStyle(el);
      if (s.visibility === 'hidden' || s.display === 'none' || parseFloat(s.opacity) === 0) {
        return null;
      }
    }
    return r;
  }

  function classify(el) {
    var tag = el.tagName.toLowerCase();
    if (tag === 'a' && el.hasAttribute('href')) return 'link';
    if (el.isContentEditable) return 'input';
    if (tag === 'textarea' || tag === 'select') return 'input';
    if (tag === 'input') {
      var t = (el.getAttribute('type') || 'text').toLowerCase();
      var clickable = t === 'button' || t === 'submit' || t === 'reset' ||
        t === 'checkbox' || t === 'radio' || t === 'file' || t === 'image';
      return clickable ? 'button' : 'input';
    }
    return 'button';
  }

  // Uniform-length labels over ALPHABET (label i is i written base-k, big-endian),
  // so labels are prefix-free and the length grows to cover any target count.
  function generateLabels(n) {
    var a = ALPHABET, k = a.length;
    if (n <= 0) return [];
    var len = 1, cap = k;
    while (cap < n) {
      len++;
      cap *= k;
    }
    var labels = [];
    for (var i = 0; i < n; i++) {
      var s = '', x = i;
      for (var d = 0; d < len; d++) {
        s = a[x % k] + s;
        x = Math.floor(x / k);
      }
      labels.push(s);
    }
    return labels;
  }

  function collect() {
    var sel = 'a[href], button, input, textarea, select, [role=button], [onclick]';
    var els = Array.prototype.slice.call(document.querySelectorAll(sel));
    var items = [];
    for (var i = 0; i < els.length; i++) {
      var rect = visibleRect(els[i]);
      if (rect) items.push({ el: els[i], rect: rect, kind: classify(els[i]) });
    }
    return items;
  }

  function teardown() {
    var o = document.getElementById(OVERLAY_ID);
    if (o) o.remove();
    state = null;
  }

  function show() {
    teardown();
    var items = collect();
    if (items.length === 0) {
      ozma.call('hintResult', { kind: 'empty' });
      return;
    }
    var labels = generateLabels(items.length);
    var overlay = document.createElement('div');
    overlay.id = OVERLAY_ID;
    overlay.setAttribute('style', 'position:fixed;left:0;top:0;width:0;height:0;z-index:2147483647;');
    var targets = [];
    for (var i = 0; i < items.length; i++) {
      var it = items[i];
      var badge = document.createElement('div');
      badge.textContent = labels[i].toUpperCase();
      badge.setAttribute('style',
        'position:fixed;left:' + Math.floor(it.rect.left) + 'px;' +
        'top:' + Math.floor(it.rect.top) + 'px;' +
        'background:#ffd76e;color:#302505;font:bold 11px/14px monospace;' +
        'padding:0 3px;border-radius:3px;box-shadow:0 1px 2px rgba(0,0,0,.4);');
      overlay.appendChild(badge);
      targets.push({ el: it.el, label: labels[i], kind: it.kind, badge: badge });
    }
    document.documentElement.appendChild(overlay);
    state = { targets: targets, prefix: '' };
  }

  function activate(t) {
    teardown();
    if (!t.el.isConnected) {
      // The captured node was detached between show() and activation (dynamic
      // page re-render); clicking it is a silent no-op, so report no activation
      // instead of a false 'navigated'/'clicked'.
      ozma.call('hintResult', { kind: 'empty' });
      return;
    }
    if (t.kind === 'input') {
      t.el.focus();
      ozma.call('hintResult', { kind: 'focusedInput' });
    } else {
      t.el.click();
      ozma.call('hintResult', { kind: t.kind === 'link' ? 'navigated' : 'clicked' });
    }
  }

  // Hides badges whose label does not start with the prefix and activates the
  // sole survivor as soon as the prefix uniquely identifies it (labels are
  // prefix-free, so a single remaining label is unambiguous even before its
  // full length is typed). Returns the number of surviving badges.
  function refilter() {
    var p = state.prefix;
    var survivor = null;
    var remaining = 0;
    for (var i = 0; i < state.targets.length; i++) {
      var t = state.targets[i];
      var hit = t.label.indexOf(p) === 0;
      t.badge.style.display = hit ? '' : 'none';
      if (hit) {
        remaining++;
        survivor = t;
      }
    }
    // NOTE: require a non-empty prefix — an empty prefix on a single-target page
    // (e.g. after a dead-end keystroke restores prefix to '') must NOT auto-activate
    // the lone hint the user never selected.
    if (remaining === 1 && p.length > 0) activate(survivor);
    return remaining;
  }

  ozma.on('hints:show', function () { show(); });
  ozma.on('hints:hide', function () { teardown(); });
  ozma.on('hints:key', function (payload) {
    if (!state) return;
    if (payload && payload.backspace) {
      if (state.prefix.length === 0) return;
      state.prefix = state.prefix.slice(0, -1);
      refilter();
      return;
    }
    var key = payload && payload.key;
    if (!key) return;
    var ch = key.toLowerCase();
    if (ALPHABET.indexOf(ch) === -1) return;
    var prev = state.prefix;
    state.prefix = prev + ch;
    if (refilter() === 0) {
      // The new prefix matches nothing — ignore the keystroke and restore the
      // previous view in a second pass (only on this rare dead-end path).
      state.prefix = prev;
      refilter();
    }
  });
})();
