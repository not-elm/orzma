// Vimium-style scroll keybindings for ozmux browser activities.
//
// Injected as a PreloadScript into each browser page webview (see
// src/browser_render.rs). Runs inside the page, owns a capture-phase keydown
// listener, and scrolls the DOM directly. No host IPC.
(function () {
  "use strict";

  // NOTE: a PreloadScript may be evaluated more than once in a single context;
  // a second listener would double every scroll, hence this idempotency guard.
  if (window.__ozmuxVimScroll) {
    return;
  }

  var LINE_STEP = 60;
  var GG_RESET_MS = 1000;

  var pendingG = false;
  var pendingGTimer = null;

  function clearPendingG() {
    pendingG = false;
    if (pendingGTimer !== null) {
      clearTimeout(pendingGTimer);
      pendingGTimer = null;
    }
  }

  function isEditableFocused() {
    var el = document.activeElement;
    if (!el) {
      return false;
    }
    var tag = el.tagName;
    if (tag === "INPUT" || tag === "TEXTAREA" || tag === "SELECT") {
      return true;
    }
    return el.isContentEditable === true;
  }

  function docCanScroll(doc, axis) {
    if (axis === "y") {
      return doc.scrollHeight > doc.clientHeight + 1;
    }
    return doc.scrollWidth > doc.clientWidth + 1;
  }

  function elementScrolls(el, axis) {
    if (!el || el.nodeType !== 1) {
      return false;
    }
    var style = window.getComputedStyle(el);
    if (axis === "y") {
      var oy = style.overflowY;
      return (oy === "auto" || oy === "scroll") && el.scrollHeight > el.clientHeight + 1;
    }
    var ox = style.overflowX;
    return (ox === "auto" || ox === "scroll") && el.scrollWidth > el.clientWidth + 1;
  }

  function scrollTarget(axis) {
    var doc = document.scrollingElement || document.documentElement;
    if (doc && docCanScroll(doc, axis)) {
      return doc;
    }
    var best = null;
    var bestArea = 0;
    var all = document.body ? document.body.querySelectorAll("*") : [];
    for (var i = 0; i < all.length; i++) {
      var el = all[i];
      if (!elementScrolls(el, axis)) {
        continue;
      }
      var area = el.clientWidth * el.clientHeight;
      if (area > bestArea) {
        bestArea = area;
        best = el;
      }
    }
    return best || doc;
  }

  function applyScroll(dx, dy) {
    var axis = dy !== 0 ? "y" : "x";
    var target = scrollTarget(axis);
    if (!target) {
      return;
    }
    target.scrollLeft += dx;
    target.scrollTop += dy;
  }

  function scrollToTop() {
    var target = scrollTarget("y");
    if (target) {
      target.scrollTop = 0;
    }
  }

  function scrollToBottom() {
    var target = scrollTarget("y");
    if (target) {
      target.scrollTop = target.scrollHeight;
    }
  }

  function decideAction(key, hadPendingG) {
    if (hadPendingG && key === "g") {
      return "top";
    }
    switch (key) {
      case "j":
        return "down";
      case "k":
        return "up";
      case "h":
        return "left";
      case "l":
        return "right";
      case "d":
        return "halfDown";
      case "u":
        return "halfUp";
      case "G":
        return "bottom";
      case "g":
        return "pendingG";
      default:
        return null;
    }
  }

  function onKeyDown(event) {
    if (event.ctrlKey || event.altKey || event.metaKey) {
      clearPendingG();
      return;
    }
    if (isEditableFocused()) {
      clearPendingG();
      return;
    }

    var hadPendingG = pendingG;
    var action = decideAction(event.key, hadPendingG);

    if (action !== "pendingG") {
      clearPendingG();
    }

    if (action === null) {
      return;
    }

    event.preventDefault();
    event.stopPropagation();

    switch (action) {
      case "down":
        applyScroll(0, LINE_STEP);
        break;
      case "up":
        applyScroll(0, -LINE_STEP);
        break;
      case "left":
        applyScroll(-LINE_STEP, 0);
        break;
      case "right":
        applyScroll(LINE_STEP, 0);
        break;
      case "halfDown": {
        var halfDownTarget = scrollTarget("y");
        if (halfDownTarget) {
          halfDownTarget.scrollTop += Math.max(1, Math.floor(halfDownTarget.clientHeight / 2));
        }
        break;
      }
      case "halfUp": {
        var halfUpTarget = scrollTarget("y");
        if (halfUpTarget) {
          halfUpTarget.scrollTop -= Math.max(1, Math.floor(halfUpTarget.clientHeight / 2));
        }
        break;
      }
      case "top":
        scrollToTop();
        break;
      case "bottom":
        scrollToBottom();
        break;
      case "pendingG":
        pendingG = true;
        pendingGTimer = setTimeout(clearPendingG, GG_RESET_MS);
        break;
    }
  }

  window.addEventListener("keydown", onKeyDown, true);

  // NOTE: window.__ozmuxVimScroll is asserted by the Rust injection test
  // (attach_injects_vim_scroll_preload in browser_render.rs); keep the name.
  window.__ozmuxVimScroll = { decideAction: decideAction };
})();
