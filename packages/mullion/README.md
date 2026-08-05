# `@lantharos/mullion`

TypeScript helpers for the Mullion page bridge (`window.mullion`).

## Install

```sh
bun add github:Lantharos/Mullion#path:packages/mullion
```

## Usage

```js
import { invoke, listen, mullion } from "@lantharos/mullion";

const version = await invoke("app.version");
listen("tray.click", () => mullion().window.show());
```

These helpers call into the bridge Mullion injects into your page. Use them from UI code that
runs inside a Mullion window.
