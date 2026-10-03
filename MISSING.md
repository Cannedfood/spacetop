# Application Compatibility Gaps

Source audit as of 2026-10-03. This is an implementation backlog, not a claim
that every listed protocol is required by every application. Most toolkits can
fall back when optional globals are absent; advertising a feature without its
behavior is often worse than not advertising it.

The target is ordinary GTK, Qt, Electron/Chromium, SDL, and X11 applications,
including their menus, dialogs, text entry, clipboard, and accelerated rendering.
Desktop shells and specialist apps require additional, explicitly scoped support.

## What Already Exists

- Core compositor/subsurface, shared-memory buffers, xdg-shell, seat, and output
  protocol plumbing through Smithay, including surface buffer scale and a fixed
  1280x720, 60 Hz virtual output.
- GLES surface-tree rendering, client DMA-BUF import and device feedback once
  the GPU is ready, and direct DMA-BUF sampling by the application's stereo
  Vulkan renderer, submitted as one OpenXR projection layer.
- Multiple native toplevels and XWayland windows, map/unmap/destroy handling,
  application-driven buffer-size changes, activation, and keyboard focus routing.
- Controller ray targeting, left/secondary click, vertical scrolling, and grip-driven
  spatial panel movement/distance adjustment. This is not client-requested
  window movement or logical window resizing.
- Native nested popup composition/grabs/repositioning/dismissal, parent-relative
  X11 menus, subsurface/input-region targeting, and expanded capture bounds.
  Menus share their parent's panel texture without changing its physical pixel scale.
  X11 parent selection uses transient-for hints and active-window/overlap fallbacks.
- Opt-in evdev keyboards and mouse buttons/wheels, XKB key/modifier delivery,
  device-disconnect cleanup, and reliable XR button transitions/tracking cleanup.
  See [README.md](README.md) for device configuration and permission requirements.
- Native Wayland clipboard offers, MIME negotiation, FD transfers, and
  keyboard-focus delivery through Smithay's `wl_data_device_manager` support.
- XR-paced dirty-panel capture, commit coalescing before rendering, popup-owner
  invalidation, and reliable input sends that do not wait for queue capacity.
  Best-effort input admission is capped, and pending frame ticks are coalesced.
- Tests for Wayland/X11 key events, native modifiers, input/focus, nested menus,
  window lifecycle, GPU pixels, subsurface/input-region targeting, expanded and
  negative popup bounds, tracking-loss releases, focus-before-click ordering,
  and clipboard transfer/focus restoration. An opt-in real-GTK startup test
  verifies that the required display interfaces allow Zenity to open.

See [src/lib.rs](src/lib.rs), [src/runtime.rs](src/runtime.rs), [src/x11.rs](src/x11.rs),
[src/gpu.rs](src/gpu.rs), [src/xr.rs](src/xr.rs), and
[TECHNICAL.md](TECHNICAL.md) for the current implementation.

## P0: Everyday App Blockers

### Keyboard Discovery And Keyboard-Free Input

- Add seat-aware automatic discovery, hotplug/reconnection, and an explicit
  host-input bridge if device access is not desirable for nested sessions.
- Provide an XR virtual keyboard for keyboard-free use, plus text-input/IME.

### Primary Selection, X11 Clipboard, And XR Drag-And-Drop

There is no primary selection or X11/Wayland selection bridge. Smithay handles
client-initiated drag-and-drop protocol/grabs, but drag icons are not rendered
and real-toolkit XR drag-and-drop remains unverified.

- Render drag icons and verify enter/motion/leave/drop, action negotiation,
  serial validation, completion/cancellation, and coordinates across XR panels.
- Verify clipboard images, large transfers, owner exit, and transfers between
  real applications; the regression covers MIME offers, text FD transfer, and
  focus restoration, not the full interoperability matrix.
- Add primary selection for middle-click paste; data-control protocols are a
  separate, permission-sensitive feature for clipboard managers.
- Bridge Wayland selections and X11 selections through Smithay's XWM support;
  test text, images, large transfers, owner exit, and cross-protocol transfers.

## P1: Broad Desktop Compatibility

### Popup Work-Area Constraints

- Define finite work-area constraints and reactive reconfiguration if an XR
  workspace needs them; placement currently uses an unconstrained spatial plane.

### Window Management And Decorations

Native xdg-shell handlers do not implement move, resize, maximize, fullscreen,
minimize, or window-menu policy. X11 move/resize handlers are empty; users cannot
request a proper logical resize.

- Implement interactive resize with valid request serials, size constraints,
  resizing state, and configure/ack sequencing. Spatial panel scaling is not a
  substitute for changing an application's logical viewport.
- Define XR policies for maximize, fullscreen, minimize/restore, close, and
  switching windows; publish consistent supported xdg-toplevel capabilities.
- Handle parented dialogs, modal/transient relationships, and focus restoration.
  Currently toplevels are placed independently without parent-aware policy.
- Add `xdg-decoration` negotiation and real server-side decorations when chosen.
  Continue allowing client-side decorations; provide usable close/resize actions
  for undecorated native and X11 windows.
- Add `xdg-activation` with token validation and a focus-stealing policy for
  application launches, links, and dialogs.

### X11 Grab, Stacking, State, And Recovery Gaps

Unowned override-redirect windows get independent panels; configure requests
ignore stacking changes.

- Refine parent inference, window-type policy, X11 grab behavior outside panel
  bounds, and stacking for unusual override-redirect applications.
- Implement XWM state requests and coherent stacking/focus policy, including
  fullscreen/maximize/minimize, normal size hints, and interactive move/resize.
- Verify managed versus override-redirect focus behavior, modal dialogs,
  WM_DELETE_WINDOW, and selection/DND interoperability with real X11 apps.
- Make XWayland failure recoverable where practical; disconnect currently
  terminates the entire compositor and therefore native Wayland apps too.

### Viewports, Output Description, And Scaling

There is no viewporter, fractional-scale, or xdg-output global, and no complete
desktop scaling model.

- Add `wp_viewporter` for source cropping and destination sizing; some rendering
  paths depend on it, while others can fall back.
- Add `xdg-output` and `wp_fractional_scale_v1` with coherent logical dimensions,
  preferred scales, and buffer sizing. Define what an output means in XR rather
  than blindly equating a per-window quad with a monitor.
- Keep surface output membership and preferred scale/transform notifications
  accurate for child surfaces and popups, not just newly created root surfaces.
- Test integer/fractional scaling, buffer transforms, viewport cropping,
  subsurface offsets, and xdg window geometry independently and in combination.
- Verify synchronized/desynchronized subsurface commits and cached state;
  Smithay supplies core handling, but the current deferred capture path
  needs regression coverage for when child state actually becomes visible.

### Cursor Support, Advanced Pointer Input, And Input Stress Testing

The cursor remains a fixed targeting cross.

- Add physical mouse-motion mapping, continuous-scroll stop information, and
  more controller-profile coverage. Simple controllers have no secondary click.
- Render client cursor surfaces with hotspots, animation, and hidden-cursor
  requests; optionally add `cursor-shape` support while retaining the XR reticle
  as a separate targeting aid.
- Add relative-pointer and pointer-constraints for games, CAD, and captured
  mouse workflows, with an explicit XR mapping and escape mechanism.
- Verify transition latency, best-effort drops, and reliable backlog growth
  during prolonged compositor stalls on the headset. Protocol tests cover
  admission limits and transition ordering, not end-to-end interaction timing.
- Touch, tablet, and gestures are optional backend-specific follow-ups, not
  prerequisites for ordinary keyboard/mouse applications.

### Presentation Feedback, Damage Tracking, And GPU Synchronization

Capture completion still does not establish actual XR presentation.
Every capture allocates a fresh linear image and waits for GLES on the CPU;
Vulkan copies every mapped panel each rendered frame and uses blocking
completion. There is no presentation-time global.

- Refine per-panel visibility/occlusion scheduling and verify callback timing
  under load. Current pacing follows renderable XR ticks, not individual
  window visibility or confirmed presentation.
- Add `wp_presentation` with truthful presented/discarded feedback tied to what
  the XR runtime can actually report. A captured or coalesced-away frame must
  not be reported as displayed.
- Pool shared images, use damage tracking, and pipeline GPU work while retaining
  correct client buffer release and consumer-completion ordering.
- Add `linux-drm-syncobj-v1` explicit synchronization where supported by the
  driver stack, especially for modern Vulkan/NVIDIA paths. Existing GLES-to-XR
  CPU waits do not implement the client explicit-sync protocol.
- Test advertised DMA-BUF formats/modifiers, rejected imports, video buffers,
  and cross-GPU clients. Do not advertise formats the renderer cannot import.
- Replace fatal mapped-window layer overflow with a capacity strategy such as
  grouping panels into a rendered scene or limiting visible layers without
  killing applications. Adding menus must not exhaust XR composition layers.

## P2: Text Input And Session Integration

- **IME and accessibility:** add text-input/input-method integration for CJK,
  preedit, candidate windows, surrounding text, and virtual keyboards. Basic
  XKB typing is separate. Accessibility also needs working session services
  such as AT-SPI; it is not solved by a Wayland global alone.
- **Portals and sandboxed apps:** provide or integrate an appropriate
  xdg-desktop-portal backend, including file chooser and URI opening. Screen
  capture/sharing needs an export mechanism plus PipeWire and portal permissions;
  consider standardized image-copy capture or compatible screencopy protocols.
- **Session environment:** define nested versus standalone deployment. Inherited
  D-Bus activation, portal services, and single-instance app routing can open
  windows on the host desktop despite correct child `DISPLAY` variables. Scope
  activation environments correctly; do not overwrite the host session globally.
- **Idle and inhibition:** support idle-inhibit for media/presentations and define
  how idle tracking and XR session visibility interact. Idle notification,
  session lock, and power policy depend on the intended session model.
- **Specialized protocols:** layer-shell, foreign-toplevel/task management,
  global shortcuts, shortcut inhibition, virtual input, and output management
  serve panels, automation, remote desktop, or desktop-shell tools. Add only
  what the intended desktop experience needs, with permissions for privileged
  operations; they are not all ordinary-app requirements.
- **Color and hardware breadth:** color-management/HDR and broader GPU/format
  compatibility are follow-ups. Current GPU sharing requirements are deliberately
  strict; broaden supported GPU paths without silently adding a CPU fallback.

Audio, networking, fonts, codecs, and application packaging are host/session
responsibilities, not implementations of the Wayland server itself.

## Verification Needed

The existing [compositor tests](tests/compositor/mod.rs) cover a small
synthetic client and selected XWayland/GPU paths. They do not establish broad
toolkit compatibility. Add focused regression cases alongside each fix and a
repeatable real-app matrix:

- GTK and Qt: typing, clipboard, context menus/submenus, combo boxes, tooltips,
  nested keyboard navigation, file dialogs, modal dialogs, resize, fullscreen,
  scaling, and close/reopen.
- Firefox and Chromium/Electron: text input/IME, context menus, clipboard/DND,
  accelerated rendering, video, downloads/dialogs, and portal screen sharing.
- SDL/native games: key transitions, relative pointer/constraints, fullscreen,
  frame pacing, and focus loss. X11 apps: repeat the common workflows through
  XWayland, including override-redirect menus and cross-protocol clipboard.
- Protocol clients: nested popups, subsurface input/commit semantics, region
  hit-testing, transforms/viewports, invalid serials, client disconnects, and
  buffer lifetime/synchronization.
- Pointer drags: verify implicit-grab coordinates across differently placed
  panels, shadows, transforms, and scaled clients with real applications.
- Physical keyboards: verify repeat, Compose, layouts, shortcuts, and full
  text-entry workflows with real toolkits/devices, beyond protocol key events.
- Hardware/headset: verify both pixels and interaction on the actual XR display,
  test channel saturation/tracking loss, and exceed the visible layer budget.

Suggested next order: real-toolkit menu/input verification -> X11 clipboard/DND ->
window-management policy and X11 semantics -> viewport/scaling -> input discovery
and pacing -> explicit sync -> IME and portal integration.
The broader matrix, rather than protocol count alone, should determine when
"almost all apps" is an accurate description.