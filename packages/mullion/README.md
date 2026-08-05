# `@lantharos/mullion`

Typed helpers for the Mullion page bridge that the host injects as `window.mullion`.

Install from Git (no npm publish required):

```sh
bun add github:Lantharos/Mullion#path:packages/mullion
```

```js
import { invoke, listen, mullion } from "@lantharos/mullion";

const version = await invoke("app.version");
listen("tray.click", () => mullion().window.show());
```

The runtime bridge still comes from the Mullion host. This package is a thin typed
facade for app UI code.
