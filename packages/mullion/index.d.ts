export type JsonValue =
  | null
  | boolean
  | number
  | string
  | JsonValue[]
  | { [key: string]: JsonValue };

export interface MullionBridge {
  readonly __native: true;
  readonly commands: string[];
  invoke(name: string, params?: Record<string, unknown>): Promise<unknown>;
  listen(name: string, callback: (payload: unknown) => void): () => void;
}

export interface MullionWindowApi {
  show(): void;
  hide(): void;
  focus(): void;
  close(): void;
  minimize(): void;
  maximize(): void;
  toggleMaximize(): void;
  restore(): void;
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
  visible?: boolean;
}

export interface MullionGuestApi {
  create(options: GuestCreateOptions): Promise<unknown>;
  destroy?(id: string): Promise<unknown>;
  setBounds?(id: string, bounds: GuestBounds): Promise<unknown>;
}

export interface MullionApi {
  bridge: MullionBridge;
  window: MullionWindowApi;
  guest?: MullionGuestApi;
  activity?: {
    begin(options?: Record<string, unknown>): Promise<{ id: string; end(): Promise<unknown> }>;
    list(): Promise<unknown>;
  };
  popup?: {
    open(options?: Record<string, unknown>): Promise<unknown>;
    close(): Promise<unknown>;
  };
}

export declare function mullion(): MullionApi;
export declare function invoke(
  name: string,
  params?: Record<string, unknown>,
): Promise<unknown>;
export declare function listen(
  name: string,
  callback: (payload: unknown) => void,
): () => void;

declare global {
  interface Window {
    mullion?: MullionApi;
  }
}

export {};
