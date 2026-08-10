export type JsonValue =
  | null
  | boolean
  | number
  | string
  | JsonValue[]
  | { [key: string]: JsonValue };

export interface SabineBridge {
  readonly __native: true;
  readonly commands: string[];
  invoke(name: string, params?: Record<string, unknown>): Promise<unknown>;
  listen(name: string, callback: (payload: unknown) => void): () => void;
}

export interface SabineWindowApi {
  show(): void;
  hide(): void;
  focus(activationToken?: string): void;
  close(): void;
  minimize(): void;
  maximize(): void;
  toggleMaximize(): void;
  setFullscreen(enabled: boolean): void;
  restore(): void;
  startDrag(): void;
}

export interface WindowFileDragEvent {
  phase: "enter" | "over" | "leave" | "drop";
  paths: string[];
  x: number;
  y: number;
  action: "copy" | "move" | "link" | "none";
  internal: boolean;
}

export interface GuestBounds {
  x: number;
  y: number;
  width: number;
  height: number;
}

export interface GuestCreateOptions {
  id?: string;
  url?: string;
  html?: string;
  bounds: GuestBounds;
  partition?: string;
  allowBridge?: boolean;
  /** Accelerators consumed while the guest is focused (e.g. `Primary+K`). */
  interceptedShortcuts?: string[];
  /** Consume predominantly horizontal wheel/trackpad input over the guest. */
  interceptHorizontalWheel?: boolean;
  visible?: boolean;
  popupPolicy?: string;
  allowDownloads?: boolean;
  backgroundColor?: string;
}

export interface GuestInfo {
  id: string;
  [key: string]: JsonValue;
}

export interface GuestNavigatedEvent {
  id: string;
  url: string;
  title: string;
  canGoBack: boolean;
  canGoForward: boolean;
}

export interface GuestNewWindowEvent {
  id: string;
  url: string;
  disposition: string;
}

export interface GuestDownloadEvent {
  guestId: string;
  downloadId: string;
  url: string;
  filename: string;
  mimeType: string;
  totalBytes: number;
  receivedBytes: number;
  state: string;
  savePath: string;
}

export interface GuestShortcutEvent {
  id: string;
  accelerator: string;
  key: string;
  repeat: boolean;
  ctrlKey: boolean;
  metaKey: boolean;
  altKey: boolean;
  shiftKey: boolean;
}

export interface GuestWheelEvent {
  id: string;
  deltaX: number;
  deltaY: number;
  ctrlKey: boolean;
  metaKey: boolean;
  altKey: boolean;
  shiftKey: boolean;
}

export interface GuestFaviconEvent {
  id: string;
  favicons: string[];
}

export interface SabineGuestApi {
  create(options: GuestCreateOptions): Promise<{ id: string } | string | unknown>;
  destroy(id: string): Promise<unknown>;
  navigate(id: string, url: string): Promise<unknown>;
  setBounds(id: string, bounds: GuestBounds): Promise<unknown>;
  setVisible(id: string, visible: boolean): Promise<unknown>;
  setCovered(covered: boolean): Promise<unknown>;
  capturePreview(id: string): Promise<unknown>;
  focus(id: string): Promise<unknown>;
  reload(id: string, options?: { ignoreCache?: boolean }): Promise<unknown>;
  goBack(id: string): Promise<unknown>;
  goForward(id: string): Promise<unknown>;
  setZoom(id: string, factor: number): Promise<unknown>;
  executeJavaScript(id: string, code: string): Promise<unknown>;
  downloadAction(
    downloadId: string,
    action: string,
    options?: { savePath?: string },
  ): Promise<unknown>;
  list(): Promise<unknown>;
  get(id: string): Promise<unknown>;
}

export interface ActivityOptions {
  label?: string;
  [key: string]: JsonValue | undefined;
}

export interface ActivityHandle {
  id: string;
  end(): Promise<unknown>;
}

export interface PopupOptions {
  x?: number;
  y?: number;
  width?: number;
  height?: number;
  html?: string;
  url?: string;
}

export interface SabineApi {
  bridge: SabineBridge;
  window: SabineWindowApi;
  guest?: SabineGuestApi;
  activity: {
    begin(options?: ActivityOptions): Promise<ActivityHandle>;
    list(): Promise<unknown>;
  };
  popup?: {
    open(options?: PopupOptions): Promise<unknown>;
    close(): Promise<unknown>;
  };
}

export declare function isAvailable(): boolean;
export declare function sabine(): SabineApi;
export declare function invoke<T = unknown>(
  name: string,
  params?: Record<string, unknown>,
): Promise<T>;
export declare function listen<T = unknown>(
  name: string,
  callback: (payload: T) => void,
): () => void;

export declare const bridge: {
  commands(): string[];
  invoke: typeof invoke;
  listen: typeof listen;
};

export declare const events: {
  fileDrag(callback: (payload: WindowFileDragEvent) => void): () => void;
  guestNavigated(callback: (payload: GuestNavigatedEvent) => void): () => void;
  guestNewWindow(callback: (payload: GuestNewWindowEvent) => void): () => void;
  guestDownload(callback: (payload: GuestDownloadEvent) => void): () => void;
  guestShortcut(callback: (payload: GuestShortcutEvent) => void): () => void;
  guestWheel(callback: (payload: GuestWheelEvent) => void): () => void;
  guestFavicon(callback: (payload: GuestFaviconEvent) => void): () => void;
};

export declare const appWindow: SabineWindowApi;

export declare class Guest {
  readonly id: string;
  constructor(id: string);
  static create(options: GuestCreateOptions): Promise<Guest>;
  get(): Promise<GuestInfo | unknown>;
  navigate(url: string): Promise<unknown>;
  setBounds(bounds: GuestBounds): Promise<unknown>;
  setVisible(visible: boolean): Promise<unknown>;
  focus(): Promise<unknown>;
  reload(options?: { ignoreCache?: boolean }): Promise<unknown>;
  goBack(): Promise<unknown>;
  goForward(): Promise<unknown>;
  setZoom(factor: number): Promise<unknown>;
  executeJavaScript(code: string): Promise<unknown>;
  capturePreview(): Promise<unknown>;
  destroy(): Promise<unknown>;
}

export declare const guest: {
  create(options: GuestCreateOptions): Promise<Guest>;
  list(): Promise<unknown>;
  get(id: string): Promise<unknown>;
  destroy(id: string): Promise<unknown>;
  navigate(id: string, url: string): Promise<unknown>;
  setBounds(id: string, bounds: GuestBounds): Promise<unknown>;
  setVisible(id: string, visible: boolean): Promise<unknown>;
  setCovered(covered: boolean): Promise<unknown>;
  focus(id: string): Promise<unknown>;
  reload(id: string, options?: { ignoreCache?: boolean }): Promise<unknown>;
  goBack(id: string): Promise<unknown>;
  goForward(id: string): Promise<unknown>;
  setZoom(id: string, factor: number): Promise<unknown>;
  executeJavaScript(id: string, code: string): Promise<unknown>;
  capturePreview(id: string): Promise<unknown>;
  downloadAction(
    downloadId: string,
    action: string,
    options?: { savePath?: string },
  ): Promise<unknown>;
};

export declare const activity: {
  begin(options?: ActivityOptions): Promise<ActivityHandle>;
  list(): Promise<unknown>;
};

export declare const popup: {
  open(options?: PopupOptions): Promise<unknown>;
  close(): Promise<unknown>;
};

declare global {
  interface Window {
    sabine?: SabineApi;
  }
}

export {};
