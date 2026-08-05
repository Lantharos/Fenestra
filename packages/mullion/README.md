# `@lantharos/mullion`

Typed helpers for pages running inside a Mullion window. Prefer these exports over reaching for
`window.mullion` directly.

## Install

```sh
bun add github:Lantharos/Mullion#path:packages/mullion
```

## Bridge commands

```js
import { invoke, listen, events, appWindow } from "@lantharos/mullion";

const { version } = await invoke("app.version");
listen("tray.click", () => appWindow.show());

events.guestDownload((download) => {
  console.log(download.filename, download.state);
});
```

## Window controls

```js
import { appWindow } from "@lantharos/mullion";

appWindow.show();
appWindow.hide();
appWindow.toggleMaximize();
```

## Guests

```js
import { guest, Guest } from "@lantharos/mullion";

const tab = await guest.create({
  url: "https://example.com",
  bounds: { x: 16, y: 64, width: 900, height: 600 },
  partition: "persist:browser",
});

await tab.navigate("https://example.com/docs");
await tab.setBounds({ x: 16, y: 64, width: 1100, height: 700 });
await tab.destroy();

// Or keep the id yourself:
const preview = await Guest.create({ html: "<h1>Hi</h1>", bounds: { x: 0, y: 0, width: 320, height: 200 } });
```

## Activity and popups

```js
import { activity, popup } from "@lantharos/mullion";

const busy = await activity.begin({ label: "Indexing" });
try {
  // …
} finally {
  await busy.end();
}

await popup.open({ x: 40, y: 80, width: 280, height: 160, html: "<p>Menu</p>" });
await popup.close();
```

## Availability

```js
import { isAvailable, mullion } from "@lantharos/mullion";

if (isAvailable()) {
  console.log(mullion().bridge.commands);
}
```

These helpers call into the bridge Mullion injects into your page. Use them from UI code that
runs inside a Mullion window.
