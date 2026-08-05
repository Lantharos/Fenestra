// Mullion bridge script. This is the single source of truth for the
// `window.mullion` JS surface that lives inside every Mullion page.
//
// The CEF host injects this script into every main frame
// after load. The host is expected to set `window.__mullionBridgeCommands`
// to a JSON array of allowed bridge command names BEFORE this script runs;
// the script copies that list into a `Set` used for `invoke()` validation.
//
// The host should also implement a navigation handler for the
// `mullion://bridge/<id>?name=<name>&payload=<payload>` scheme used by
// `invoke()`. The host parses the URL, dispatches to its registered bridge
// handler, then calls `window.__mullionBridgeResolve(id, ok, payload)` from
// the host side to resolve the promise returned by `invoke()`.
//
// The host injects bridge events by calling
// `window.__mullionBridgeEmit(name, payload)` from the host side.
// `window.mullion.window.*` calls navigate to `mullion://window/<action>`,
// which the host interprets as a host control (show/hide/focus/close/etc.)
// rather than a bridge command.
//
// This file is included as a `&str` from Rust via `include_str!`, embedded
// into the C++ CEF host as a generated header at build time, and posted
// Do not duplicate the body elsewhere; always edit this file.

(function () {
  if (window.mullion && window.mullion.bridge && window.mullion.bridge.__native) return;
  const commands = new Set(window.__mullionBridgeCommands || []);
  const pending = new Map();
  const listeners = new Map();
  let nextId = 1;

  window.__mullionBridgeResolve = function (id, ok, payload) {
    const entry = pending.get(String(id));
    if (!entry) return;
    pending.delete(String(id));
    if (ok) {
      entry.resolve(payload);
    } else {
      entry.reject(new Error((payload && payload.message) || "Mullion bridge command failed"));
    }
  };

  window.__mullionBridgeEmit = function (name, payload) {
    const set = listeners.get(String(name));
    if (set) {
      for (const cb of Array.from(set)) {
        queueMicrotask(() => cb(payload));
      }
    }
    window.dispatchEvent(new CustomEvent("mullion:" + String(name), { detail: payload }));
  };

  const encodeQuery = function (params) {
    const entries = [];
    for (const key of Object.keys(params || {})) {
      const value = params[key];
      if (value === undefined || value === null) continue;
      entries.push(encodeURIComponent(key) + "=" + encodeURIComponent(String(value)));
    }
    return entries.length ? "?" + entries.join("&") : "";
  };

  const windowCommand = function (action, params) {
    const url =
      "mullion://window/" +
      action +
      encodeQuery(Object.assign({ at: Date.now() + "-" + Math.random() }, params || {}));
    window.location.href = url;
  };

  window.mullion = window.mullion || {};
  window.mullion.window = Object.assign(window.mullion.window || {}, {
    show() { windowCommand("show"); },
    hide() { windowCommand("hide"); },
    focus() { windowCommand("focus"); },
    close() { windowCommand("close"); },
    minimize() { windowCommand("minimize"); },
    maximize() { windowCommand("maximize"); },
    toggleMaximize() { windowCommand("toggle-maximize"); },
    restore() { windowCommand("restore"); },
  });

  window.mullion.bridge = {
    __native: true,
    commands: Array.from(commands),
    listen(name, callback) {
      const key = String(name);
      let set = listeners.get(key);
      if (!set) { set = new Set(); listeners.set(key, set); }
      set.add(callback);
      return () => {
        set.delete(callback);
        if (!set.size) listeners.delete(key);
      };
    },
    invoke(name, params = {}) {
      if (!commands.has(name)) {
        return Promise.reject(new Error("Mullion bridge command not registered: " + name));
      }
      const id = String(nextId++);
      const payload = encodeURIComponent(JSON.stringify(params));
      const url =
        "mullion://bridge/" +
        encodeURIComponent(id) +
        "?name=" + encodeURIComponent(name) +
        "&payload=" + payload;
      return new Promise((resolve, reject) => {
        pending.set(id, { resolve, reject });
        setTimeout(() => {
          if (pending.has(id)) {
            pending.delete(id);
            reject(new Error("Mullion bridge command timed out: " + name));
          }
        }, 60000);
        window.location.href = url;
      });
    },
  };

  window.mullion.activity = {
    begin(options = {}) {
      return window.mullion.bridge.invoke("mullion.activity.begin", options).then((record) => {
        let ended = false;
        return Object.assign({}, record, {
          end() {
            if (ended) return Promise.resolve({ id: record.id, ended: false });
            ended = true;
            return window.mullion.bridge.invoke("mullion.activity.end", { id: record.id });
          },
        });
      });
    },
    list() { return window.mullion.bridge.invoke("mullion.activity.list"); },
  };

  if (commands.has("mullion.popup.open") && commands.has("mullion.popup.close")) {
    window.mullion.popup = Object.assign(window.mullion.popup || {}, {
      open(options = {}) {
        return window.mullion.bridge.invoke("mullion.popup.open", {
          x: Math.round(Number(options.x) || 0),
          y: Math.round(Number(options.y) || 0),
          width: Math.max(1, Math.round(Number(options.width) || 1)),
          height: Math.max(1, Math.round(Number(options.height) || 1)),
          html: String(options.html || ""),
          url: String(options.url || ""),
        });
      },
      close() {
        return window.mullion.bridge.invoke("mullion.popup.close");
      },
    });
  }

  if (commands.has("mullion.guest.create")) {
    const guestBounds = function (options) {
      const bounds = options.bounds || options;
      return {
        x: Math.round(Number(bounds.x) || 0),
        y: Math.round(Number(bounds.y) || 0),
        width: Math.max(1, Math.round(Number(bounds.width) || 1)),
        height: Math.max(1, Math.round(Number(bounds.height) || 1)),
      };
    };

    window.mullion.guest = Object.assign(window.mullion.guest || {}, {
      create(options = {}) {
        const bounds = guestBounds(options);
        return window.mullion.bridge.invoke("mullion.guest.create", {
          id: options.id ? String(options.id) : undefined,
          url: options.url ? String(options.url) : undefined,
          html: options.html ? String(options.html) : undefined,
          x: bounds.x,
          y: bounds.y,
          width: bounds.width,
          height: bounds.height,
          bounds,
          partition: options.partition ? String(options.partition) : undefined,
          allowBridge: Boolean(options.allowBridge),
          visible: options.visible === undefined ? true : Boolean(options.visible),
          popupPolicy: String(options.popupPolicy || "deny"),
          allowDownloads:
            options.allowDownloads === undefined ? true : Boolean(options.allowDownloads),
          backgroundColor: options.backgroundColor
            ? String(options.backgroundColor)
            : undefined,
        });
      },
      destroy(id) {
        return window.mullion.bridge.invoke("mullion.guest.destroy", { id: String(id) });
      },
      navigate(id, url) {
        return window.mullion.bridge.invoke("mullion.guest.navigate", {
          id: String(id),
          url: String(url),
        });
      },
      setBounds(id, bounds) {
        const next = guestBounds(bounds || {});
        return window.mullion.bridge.invoke("mullion.guest.setBounds", {
          id: String(id),
          x: next.x,
          y: next.y,
          width: next.width,
          height: next.height,
          bounds: next,
        });
      },
      setVisible(id, visible) {
        return window.mullion.bridge.invoke("mullion.guest.setVisible", {
          id: String(id),
          visible: Boolean(visible),
        });
      },
      setCovered(covered) {
        return window.mullion.bridge.invoke("mullion.guest.setCovered", {
          covered: Boolean(covered),
        });
      },
      capturePreview(id) {
        return window.mullion.bridge.invoke("mullion.guest.capturePreview", {
          id: String(id),
        });
      },
      focus(id) {
        return window.mullion.bridge.invoke("mullion.guest.focus", { id: String(id) });
      },
      reload(id, options = {}) {
        return window.mullion.bridge.invoke("mullion.guest.reload", {
          id: String(id),
          ignoreCache: Boolean(options.ignoreCache),
        });
      },
      goBack(id) {
        return window.mullion.bridge.invoke("mullion.guest.goBack", { id: String(id) });
      },
      goForward(id) {
        return window.mullion.bridge.invoke("mullion.guest.goForward", { id: String(id) });
      },
      setZoom(id, factor) {
        return window.mullion.bridge.invoke("mullion.guest.setZoom", {
          id: String(id),
          factor: Number(factor) || 1,
        });
      },
      executeJavaScript(id, code) {
        return window.mullion.bridge.invoke("mullion.guest.executeJavaScript", {
          id: String(id),
          code: String(code),
        });
      },
      downloadAction(downloadId, action, options = {}) {
        return window.mullion.bridge.invoke("mullion.guest.downloadAction", {
          downloadId: String(downloadId),
          action: String(action),
          savePath: options.savePath ? String(options.savePath) : undefined,
        });
      },
      list() {
        return window.mullion.bridge.invoke("mullion.guest.list");
      },
      get(id) {
        return window.mullion.bridge.invoke("mullion.guest.get", { id: String(id) });
      },
    });
  }
})();
