/** @typedef {import("./index.d.ts").MullionBridge} MullionBridge */

function requireBridge() {
  const bridge = globalThis.window?.mullion?.bridge;
  if (!bridge?.__native) {
    throw new Error(
      "Mullion bridge is not available. This package only works inside a Mullion window.",
    );
  }
  return bridge;
}

/** @returns {import("./index.d.ts").MullionApi} */
export function mullion() {
  const api = globalThis.window?.mullion;
  if (!api) {
    throw new Error(
      "window.mullion is missing. This package only works inside a Mullion window.",
    );
  }
  return api;
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

export default {
  mullion,
  invoke,
  listen,
};
