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
  protocol plumbing through Smithay.
- GLES surface-tree rendering, client DMA-BUF import and device feedback once
  the GPU is ready, and GPU sharing into per-window OpenXR swapchains.
- Multiple native toplevels and XWayland windows, map/unmap/destroy handling,
  application-driven buffer-size changes, activation, and keyboard focus routing.
- Controller ray targeting, left click, vertical scrolling, and grip-driven
  spatial panel movement/distance adjustment. This is not client-requested
  window movement or logical window resizing.
- Tests for basic input/focus, window lifecycle, GPU pixels, and XWayland.

See [src/main.rs](src/main.rs), [src/x11.rs](src/x11.rs),
[src/gpu.rs](src/gpu.rs), [src/xr.rs](src/xr.rs), and
[TECHNICAL.md](TECHNICAL.md) for the current implementation.

## P0: Everyday App Blockers

### Real Keyboard Input

The seat advertises a keyboard and sets focus, but there is no production source
of key press/release events. `XrInput` has no keyboard event variant. The X11
keyboard target forwards events if supplied; it does not acquire them.

- Add a physical keyboard input backend or an explicit host-input bridge;
  provide an XR virtual keyboard if keyboard-free use is a goal.
- Deliver keycodes and modifier transitions through Smithay's keyboard handling,
  with configurable XKB layouts/options, repeat, Compose, and focus-loss cleanup.
- Test typing, shortcuts, held modifiers, repeat, and focus switches in both
  native Wayland and X11 apps. Keyboard-enter events alone do not prove typing.

### Popups, Menus, And Tooltips

`new_popup` only sends a configure, popup grabs are ignored, and repositioning
only acknowledges the token. Capture renders a toplevel's subsurface tree, not
its separate xdg-popup tree. Popup commits do not resolve to their owning panel
through the subsurface-parent walk.

- Track popup ownership, nested popups, lifecycle, positioner geometry,
  constraint adjustment, reactive placement, and reposition configures.
- Render them relative to their parent, including content outside the parent's
  buffer bounds, and translate input coordinates correctly.
- Implement serial-validated popup grabs, keyboard/pointer routing, dismissal,
  and `popup_done` handling.
- Test context menus, combo boxes, submenus, tooltips, and popups at every edge.

### Surface-Aware Pointer Targeting

Ray targeting uses the root surface's rectangular buffer bounds and always
focuses that root. Rendering subsurfaces does not make them independently
clickable. Input regions, child stacking, and child coordinate offsets are not
consulted by this hit-test.

- Hit-test the actual surface hierarchy, including popups and input regions.
- Respect xdg window geometry and distinguish content coordinates, buffer
  coordinates, and the bounds of the composed tree. Shadows and negative child
  offsets must not shift clicks or cause unintended clipping.
- Preserve pointer-grab behavior for drags across panel boundaries and verify
  press/release delivery to the right surface.
- Test a clickable subsurface, input-region holes, shadows, and drag-outside.

### Clipboard, Selection, And Drag-And-Drop

No `wl_data_device_manager` is initialized, and there are no compositor
selection handlers or X11/Wayland selection bridge.

- Implement regular clipboard offers, ownership, MIME negotiation, streaming,
  cancellation, and selection delivery when keyboard focus changes.
- Implement drag-and-drop enter/motion/leave/drop, action negotiation, serial
  validation, drag icons, and completion/cancellation.
- Add primary selection for middle-click paste; data-control protocols are a
  separate, permission-sensitive feature for clipboard managers.
- Bridge Wayland selections and X11 selections through Smithay's XWM support;
  test text, images, large transfers, owner exit, and cross-protocol transfers.

## P1: Broad Desktop Compatibility

### Window Management And Decorations

Native xdg-shell handlers do not implement move, resize, maximize, fullscreen,
minimize, or window-menu policy. X11 move/resize handlers are empty. A new client
buffer size is accepted, but users cannot request a proper logical resize.

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

### X11 Menus And Window-Manager Semantics

Rootless XWayland is already supported; replacing it is not the missing piece.
Override-redirect windows currently become independent panels, and configure
requests ignore stacking changes.

- Position X11 menus/tooltips relative to their owning window instead of giving
  them unrelated spatial placement. Honor transient-for and window-type hints.
- Implement XWM state requests and coherent stacking/focus policy, including
  fullscreen/maximize/minimize, normal size hints, and interactive move/resize.
- Verify managed versus override-redirect focus behavior, modal dialogs,
  WM_DELETE_WINDOW, and selection/DND interoperability with real X11 apps.
- Make XWayland failure recoverable where practical; disconnect currently
  terminates the entire compositor and therefore native Wayland apps too.

### Viewports, Output Description, And Scaling

Only a fixed 1280x720, 60 Hz virtual output is created. There is no viewporter,
fractional-scale, or xdg-output global. Surface buffer scale exists, but it is
not a complete desktop scaling model.

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
  Smithay supplies core handling, but the current commit-triggered capture path
  needs regression coverage for when child state actually becomes visible.

### Complete Pointer Behavior And Reliable Input Delivery

Only left click and vertical continuous scrolling are emitted. The displayed
cursor is a fixed targeting cross, not the app's requested cursor.

- Add right/middle/extra buttons, horizontal scroll, and appropriate scroll
  source/stop information. Supply discrete/value120 events for wheel input
  when that backend is available.
- Render client cursor surfaces with hotspots, animation, and hidden-cursor
  requests; optionally add `cursor-shape` support while retaining the XR reticle
  as a separate targeting aid.
- Add relative-pointer and pointer-constraints for games, CAD, and captured
  mouse workflows, with an explicit XR mapping and escape mechanism.
- Do not drop button/key transitions. The bounded XR input channel uses ignored
  `try_send` results, so a lost release can leave a button held. Coalesce motion,
  preserve transitions, and clear state on tracking/session/focus loss.
- Touch, tablet, and gestures are optional backend-specific follow-ups, not
  prerequisites for ordinary keyboard/mouse applications.

### Frame Pacing, Presentation, And GPU Synchronization

Frame callbacks complete immediately after panel capture, before XR presentation.
Every capture allocates a fresh linear image and waits for GLES on the CPU;
Vulkan copies also use blocking completion. There is no presentation-time global.

- Schedule redraw/callback delivery against XR cadence and visibility, rather
  than letting commit rate determine animation pace and allocation pressure.
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

The existing [src/compositor_tests.rs](src/compositor_tests.rs) covers a small
synthetic client and selected XWayland/GPU paths. It does not establish broad
toolkit compatibility. Add focused regression cases alongside each fix and a
repeatable real-app matrix:

- GTK and Qt: typing, clipboard, menus/submenus, tooltips, file dialogs, modal
  dialogs, resize, fullscreen, scaling, and close/reopen.
- Firefox and Chromium/Electron: text input/IME, context menus, clipboard/DND,
  accelerated rendering, video, downloads/dialogs, and portal screen sharing.
- SDL/native games: key transitions, relative pointer/constraints, fullscreen,
  frame pacing, and focus loss. X11 apps: repeat the common workflows through
  XWayland, including override-redirect menus and cross-protocol clipboard.
- Protocol clients: nested popups, subsurface input/commit semantics, region
  hit-testing, transforms/viewports, invalid serials, client disconnects, and
  buffer lifetime/synchronization.
- Hardware/headset: verify both pixels and interaction on the actual XR display,
  test channel saturation/tracking loss, and exceed the visible layer budget.

Suggested order: keyboard -> popup composition and surface hit-testing ->
clipboard/DND -> window-management policy and X11 transients -> viewport/scaling
-> reliable input and pacing -> explicit sync -> IME and portal integration.
The broader matrix, rather than protocol count alone, should determine when
"almost all apps" is an accurate description.