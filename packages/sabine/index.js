/** @typedef {import("./index.d.ts").SabineBridge} SabineBridge */
/** @typedef {import("./index.d.ts").SabineApi} SabineApi */
/** @typedef {import("./index.d.ts").GuestBounds} GuestBounds */
/** @typedef {import("./index.d.ts").GuestCreateOptions} GuestCreateOptions */
/** @typedef {import("./index.d.ts").GuestInfo} GuestInfo */
/** @typedef {import("./index.d.ts").ActivityOptions} ActivityOptions */
/** @typedef {import("./index.d.ts").PopupOptions} PopupOptions */

function requireApi() {
  const api = globalThis.window?.sabine;
  if (!api) {
    throw new Error(
      "window.sabine is missing. This package only works inside a Sabine window.",
    );
  }
  return api;
}

function requireBridge() {
  const bridge = requireApi().bridge;
  if (!bridge?.__native) {
    throw new Error(
      "Sabine bridge is not available. This package only works inside a Sabine window.",
    );
  }
  return bridge;
}

function requireGuestApi() {
  const guest = requireApi().guest;
  if (!guest?.create) {
    throw new Error(
      "Sabine guests are not enabled for this window. Register guest bridge commands on the host.",
    );
  }
  return guest;
}

/** @returns {boolean} */
export function isAvailable() {
  return Boolean(globalThis.window?.sabine?.bridge?.__native);
}

/** @returns {SabineApi} */
export function sabine() {
  return requireApi();
}

/**
 * @param {string} name
 * @param {Record<string, unknown>} [params]
 */
export function invoke(name, params = {}) {
  return requireBridge().invoke(name, params);
}

/**
 * @param {string} name
 * @param {(payload: unknown) => void} callback
 */
export function listen(name, callback) {
  return requireBridge().listen(name, callback);
}

export const bridge = {
  /** @returns {string[]} */
  commands() {
    return requireBridge().commands.slice();
  },
  invoke,
  listen,
};

export const events = {
  /** @param {(payload: import("./index.d.ts").GuestNavigatedEvent) => void} callback */
  guestNavigated(callback) {
    return listen("guest.navigated", callback);
  },
  /** @param {(payload: import("./index.d.ts").GuestNewWindowEvent) => void} callback */
  guestNewWindow(callback) {
    return listen("guest.newWindow", callback);
  },
  /** @param {(payload: import("./index.d.ts").GuestDownloadEvent) => void} callback */
  guestDownload(callback) {
    return listen("guest.download", callback);
  },
  /** @param {(payload: import("./index.d.ts").GuestShortcutEvent) => void} callback */
  guestShortcut(callback) {
    return listen("guest.shortcut", callback);
  },
  /** @param {(payload: import("./index.d.ts").GuestWheelEvent) => void} callback */
  guestWheel(callback) {
    return listen("guest.wheel", callback);
  },
  /** @param {(payload: import("./index.d.ts").GuestFaviconEvent) => void} callback */
  guestFavicon(callback) {
    return listen("guest.favicon", callback);
  },
};

export const appWindow = {
  show() {
    requireApi().window.show();
  },
  hide() {
    requireApi().window.hide();
  },
  focus() {
    requireApi().window.focus();
  },
  close() {
    requireApi().window.close();
  },
  minimize() {
    requireApi().window.minimize();
  },
  maximize() {
    requireApi().window.maximize();
  },
  toggleMaximize() {
    requireApi().window.toggleMaximize();
  },
  restore() {
    requireApi().window.restore();
  },
};

/**
 * Handle for a guest surface created through the Sabine bridge.
 */
export class Guest {
  /** @param {string} id */
  constructor(id) {
    this.id = String(id);
  }

  /**
   * @param {GuestCreateOptions} options
   * @returns {Promise<Guest>}
   */
  static async create(options) {
    const result = /** @type {{ id?: string } | string | null} */ (
      await requireGuestApi().create(options)
    );
    const id =
      typeof result === "string"
        ? result
        : result && typeof result === "object" && "id" in result && result.id
          ? String(result.id)
          : options.id
            ? String(options.id)
            : null;
    if (!id) {
      throw new Error("Sabine guest.create did not return an id");
    }
    return new Guest(id);
  }

  /** @returns {Promise<GuestInfo | unknown>} */
  get() {
    return requireGuestApi().get(this.id);
  }

  /** @param {string} url */
  navigate(url) {
    return requireGuestApi().navigate(this.id, url);
  }

  /** @param {GuestBounds} bounds */
  setBounds(bounds) {
    return requireGuestApi().setBounds(this.id, bounds);
  }

  /** @param {boolean} visible */
  setVisible(visible) {
    return requireGuestApi().setVisible(this.id, visible);
  }

  focus() {
    return requireGuestApi().focus(this.id);
  }

  /** @param {{ ignoreCache?: boolean }} [options] */
  reload(options = {}) {
    return requireGuestApi().reload(this.id, options);
  }

  goBack() {
    return requireGuestApi().goBack(this.id);
  }

  goForward() {
    return requireGuestApi().goForward(this.id);
  }

  /** @param {number} factor */
  setZoom(factor) {
    return requireGuestApi().setZoom(this.id, factor);
  }

  /** @param {string} code */
  executeJavaScript(code) {
    return requireGuestApi().executeJavaScript(this.id, code);
  }

  capturePreview() {
    return requireGuestApi().capturePreview(this.id);
  }

  destroy() {
    return requireGuestApi().destroy(this.id);
  }
}

export const guest = {
  /**
   * @param {GuestCreateOptions} options
   * @returns {Promise<Guest>}
   */
  create(options) {
    return Guest.create(options);
  },
  /** @returns {Promise<unknown>} */
  list() {
    return requireGuestApi().list();
  },
  /** @param {string} id */
  get(id) {
    return requireGuestApi().get(id);
  },
  /** @param {string} id */
  destroy(id) {
    return requireGuestApi().destroy(id);
  },
  /**
   * @param {string} id
   * @param {string} url
   */
  navigate(id, url) {
    return requireGuestApi().navigate(id, url);
  },
  /**
   * @param {string} id
   * @param {GuestBounds} bounds
   */
  setBounds(id, bounds) {
    return requireGuestApi().setBounds(id, bounds);
  },
  /**
   * @param {string} id
   * @param {boolean} visible
   */
  setVisible(id, visible) {
    return requireGuestApi().setVisible(id, visible);
  },
  /** @param {boolean} covered */
  setCovered(covered) {
    return requireGuestApi().setCovered(covered);
  },
  /** @param {string} id */
  focus(id) {
    return requireGuestApi().focus(id);
  },
  /**
   * @param {string} id
   * @param {{ ignoreCache?: boolean }} [options]
   */
  reload(id, options = {}) {
    return requireGuestApi().reload(id, options);
  },
  /** @param {string} id */
  goBack(id) {
    return requireGuestApi().goBack(id);
  },
  /** @param {string} id */
  goForward(id) {
    return requireGuestApi().goForward(id);
  },
  /**
   * @param {string} id
   * @param {number} factor
   */
  setZoom(id, factor) {
    return requireGuestApi().setZoom(id, factor);
  },
  /**
   * @param {string} id
   * @param {string} code
   */
  executeJavaScript(id, code) {
    return requireGuestApi().executeJavaScript(id, code);
  },
  /** @param {string} id */
  capturePreview(id) {
    return requireGuestApi().capturePreview(id);
  },
  /**
   * @param {string} downloadId
   * @param {string} action
   * @param {{ savePath?: string }} [options]
   */
  downloadAction(downloadId, action, options = {}) {
    return requireGuestApi().downloadAction(downloadId, action, options);
  },
};

export const activity = {
  /**
   * @param {ActivityOptions} [options]
   * @returns {Promise<{ id: string, end(): Promise<unknown> }>}
   */
  begin(options = {}) {
    return requireApi().activity.begin(options);
  },
  list() {
    return requireApi().activity.list();
  },
};

export const popup = {
  /**
   * @param {PopupOptions} [options]
   */
  open(options = {}) {
    const api = requireApi().popup;
    if (!api?.open) {
      throw new Error("Sabine popups are not enabled for this window.");
    }
    return api.open(options);
  },
  close() {
    const api = requireApi().popup;
    if (!api?.close) {
      throw new Error("Sabine popups are not enabled for this window.");
    }
    return api.close();
  },
};

export default {
  isAvailable,
  sabine,
  invoke,
  listen,
  bridge,
  events,
  appWindow,
  Guest,
  guest,
  activity,
  popup,
};
