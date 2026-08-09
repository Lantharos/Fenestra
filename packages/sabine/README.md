# `@lantharos/sabine`

Typed helpers for pages running inside a Sabine window. Prefer these exports over reaching for
`window.sabine` directly.

## Install

```sh
bun add github:Lantharos/Sabine#path:packages/sabine
```

## Bridge commands

```js
import { invoke, listen, events, appWindow } from "@lantharos/sabine";

const { version } = await invoke("app.version");
listen("tray.click", () => appWindow.show());

events.guestDownload((download) => {
  console.log(download.filename, download.state);
});

events.guestShortcut((event) => {
  console.log(event.accelerator, event.key);
});

events.guestWheel((event) => {
  console.log(event.deltaX, event.deltaY);
});

events.guestFavicon((event) => {
  console.log(event.id, event.favicons);
});

events.fileDrag((event) => {
  console.log(event.phase, event.paths, event.x, event.y, event.action);
});
```

## Window controls

```js
import { appWindow } from "@lantharos/sabine";

appWindow.show();
appWindow.hide();
appWindow.toggleMaximize();
appWindow.startDrag();
```

## Guests

```js
import { guest, Guest } from "@lantharos/sabine";

const tab = await guest.create({
  url: "https://example.com",
  bounds: { x: 16, y: 64, width: 900, height: 600 },
  partition: "persist:browser",
  interceptedShortcuts: ["Primary+T", "Primary+K"],
  interceptHorizontalWheel: true,
});

await tab.navigate("https://example.com/docs");
await tab.setBounds({ x: 16, y: 64, width: 1100, height: 700 });
await tab.destroy();

// Or keep the id yourself:
const preview = await Guest.create({ html: "<h1>Hi</h1>", bounds: { x: 0, y: 0, width: 320, height: 200 } });
```

## Activity and popups

```js
import { activity, popup } from "@lantharos/sabine";

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
import { isAvailable, sabine } from "@lantharos/sabine";

if (isAvailable()) {
  console.log(sabine().bridge.commands);
}
```

These helpers call into the bridge Sabine injects into your page. Use them from UI code that
runs inside a Sabine window.
