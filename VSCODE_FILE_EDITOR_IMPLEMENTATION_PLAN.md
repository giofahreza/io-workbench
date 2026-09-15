# Native VS Code-Style File Explorer & Editor Plan

**Status:** Proposed — no implementation has started for this plan.

## Goal

Add a second, native Android Files-page presentation that feels like the VS Code
Explorer while retaining the current detailed file list.

| View | Purpose |
| --- | --- |
| **Details** | Keep the existing table with name, size, modified time, bulk selection, and toolbar actions. |
| **Explorer** | Show a compact, expandable project tree with VS Code Seti file icons, selected-file styling, and an editor-oriented layout. |

The feature is a **native Android UI**. It must not embed VS Code, Electron,
Monaco, code-server, OpenVSCode Server, or a WebView for the explorer or code
editor.

## Architectural Decisions

1. **Keep the server as the file authority.**
   - Remote and Termux projects remain server-backed through the existing
     `MobileController.listFolderEntries()` and file mutation APIs.
   - The server supplies file data only; it does not render the Explorer UI.

2. **Keep Termux as an external host boundary.**
   - The Android app cannot directly inspect Termux private project files.
   - The local-Termux mode therefore continues to use authenticated localhost
     APIs, exactly like a remote project uses its remote server APIs.

3. **Use native Android Views.**
   - Continue with `RecyclerView`, `ListAdapter`, `DiffUtil`, and the existing
     native `NativeCodeEditorView`.
   - Use AndroidX `SlidingPaneLayout` for an adaptive Explorer-plus-editor
     layout instead of introducing Compose only for this feature.

4. **Reuse existing assets and actions.**
   - The app already includes the VS Code Seti icon font/theme.
   - Existing open, create, rename, copy, cut, paste, upload, delete, media,
     selection, keyboard shortcut, and Git behaviors remain the one source of
     truth for both views.

5. **Do not present this as actual VS Code.**
   - The UI can be called **Explorer** in the product and described as
     "VS Code-style" in documentation.
   - Extension marketplace, debugger adapters, Electron desktop behaviors, and
     browser-only VS Code components are explicitly out of scope.

## Existing Building Blocks

| Area | Current implementation | Reuse in Explorer |
| --- | --- | --- |
| Files screen | `androidApp/src/main/kotlin/io/workbench/mobile/ui/workspace-git.kt` | Native `RecyclerView` shell and file toolbar host. |
| Tree projection | `androidApp/src/main/kotlin/io/workbench/mobile/ui/files-media-editor.kt` | Existing flattened rows, lazy folder cache, expansion state, stable IDs, and scroll-anchor restoration. |
| File row actions | `androidApp/src/main/kotlin/io/workbench/mobile/ui/workspace-rendering.kt` | Click, long-press context menu, selection, media preview, and editor opening. |
| Seti icons | `androidApp/src/main/kotlin/io/workbench/mobile/ui/rendering-and-formatting.kt` | File-type icon rendering already matches VS Code conventions. |
| File API | `shared/src/commonMain/kotlin/io/workbench/mobile/shared/controller/files-and-git.kt` | Lazy child-directory queries and all existing mutation operations. |
| Native editor | `androidApp/src/main/kotlin/io/workbench/mobile/NativeCodeEditorView.kt` | Detail pane/editor rendering; do not replace with Monaco or a WebView. |

## User Experience

### View Switch

- Add a compact, accessible two-choice control in the Files toolbar or overflow
  menu: **Details** and **Explorer**.
- Preserve the selected view in Android preferences. Details remains the
  initial default so existing users see no unexpected workflow change.
- A change of presentation must update only the Files screen. It must not
  reconnect, reload unrelated tabs, clear chat state, or refresh every page.

### Details View

- Preserve the current behavior and metadata columns.
- Preserve the current expanded-folder, bulk-selection, upload, create, paste,
  refresh, and context-menu behavior.
- Do not change the bottom-left chat-session provider/CLI icon; that control
  remains independent of this feature.

### Explorer View

- Show the selected project as the root header.
- Render a compact single-column tree:
  - disclosure chevron for folders;
  - folder or Seti file icon;
  - filename;
  - selected/open-file state;
  - loading indicator or inline error when a folder request is in flight or
    fails.
- Hide Details-only size and modified-time columns to make the tree easy to
  scan on a phone.
- Tap a folder to expand/collapse it. Tap a file to open the existing native
  editor. Long press opens the same context menu as Details.
- Keep multi-select and file mutations available through the existing toolbar
  and context menu rather than inventing a separate action model.
- Include explicit Refresh and Collapse all actions in the Explorer overflow
  menu. Refresh invalidates only the project tree cache; Collapse all changes
  local UI state only.

### Small Screens, Large Screens, and Foldables

- **Phones in portrait:** Explorer occupies the Files page. Opening a file
  shows the native editor; Back or an Explorer button returns to the tree.
- **Landscape phones, tablets, and foldables:** use `SlidingPaneLayout` to
  show a persistent Explorer pane beside the native editor when width permits.
- **Narrow layouts:** `SlidingPaneLayout` automatically overlays the editor;
  the system Back action returns to the Explorer pane before navigating away.
- Never force a desktop-sized three-column VS Code layout onto a phone.

## Data, State, and Refresh Design

### Persistent UI State

Introduce Android-only presentation state, scoped by server profile and
project:

- `FilesViewMode`: `DETAILS` or `EXPLORER`.
- Last expanded folder paths.
- Last scroll anchor per view.
- Selected/open file path for Explorer highlighting.
- In-flight folder requests, request generations, and inline error state.

Do not place this visual-only state in the server protocol or project metadata.

### Tree Projection

Use a flattened list of immutable native render rows, not recursively nested
Android Views:

1. Start at the project root (`.`) for Explorer mode.
2. Render only root children plus children of expanded folders.
3. Load a folder only after its first expansion through
   `controller.listFolderEntries()`.
4. Preserve stable IDs based on project and normalized relative path.
5. Submit row deltas with the existing `ListAdapter`/`DiffUtil` path so an
   expansion does not rebuild unrelated rows or pages.
6. Cancel or ignore a child request if the user collapses the folder, switches
   project/server, leaves Files, or receives a newer request generation.

### File Events and Mutations

- Reuse the current file-change event handling and dirty-folder invalidation.
- On create, delete, rename, copy, cut, paste, upload, or server event,
  invalidate the affected parent directory only.
- Reload an affected directory only if it is visible or expanded; otherwise
  mark it dirty and load it on the next expansion.
- Preserve the active editor unless the active file was renamed or deleted.
- Never treat an Explorer refresh as an app-wide `render()` reset.

## Optional Future: Device-Local Workspaces

This is a separate feature from the native Explorer presentation.

- An Android-local workspace can use `ACTION_OPEN_DOCUMENT_TREE`, persisted URI
  grants, `ContentResolver`, and `DocumentFile` for a user-selected shared
  folder.
- It must be labeled **Device folder**, not a Termux or server project.
- Android's Storage Access Framework does not grant access to Termux's private
  sandbox, and the Termux app does not inherit this app's URI grants.
- A Device-folder workspace therefore needs separate native read/write,
  import/export, and conflict behavior before it can participate in server-side
  Git, terminal, agent, or database workflows.

Do not include this optional source in the first Explorer implementation.

## Implementation Stages

### 1. Define Presentation State

- Add `FilesViewMode` and preference keys in the Android app.
- Add an explicit `Files` screen signature that includes view mode without
  invalidating other tabs.
- Define project/server-scoped cache keys and lifecycle cleanup behavior.

### 2. Extract Shared File-Tree Model

- Move the existing flattened tree state into a small Android-only explorer
  model/controller so Details and Explorer share loading, selection, cache, and
  mutation behavior.
- Keep server DTOs and KMP controller APIs unchanged.
- Add unit tests for row flattening, tree state restoration, and targeted cache
  invalidation.

### 3. Build the Native Explorer Renderer

- Add Explorer-specific rows and a compact native ViewHolder.
- Reuse `fileTypeIcon()` for Seti icons and existing context actions.
- Add accessible content descriptions for expanded/collapsed folders, loading
  folders, and the active file.
- Add Collapse all, Refresh, and view-switch actions.

### 4. Integrate the Native Editor

- Preserve the current editor on file open and save.
- Add Explorer active-file highlighting and automatic reveal of the active file
  when the tree has enough cached path information.
- Keep single-document behavior initially; multiple editor tabs are a separate
  follow-up, not a prerequisite for Explorer mode.

### 5. Add Adaptive Two-Pane Support

- Integrate `SlidingPaneLayout` only around the Files/Explorer-and-editor
  surface.
- Verify portrait overlay behavior, landscape side-by-side behavior, foldable
  hinge handling, and Android Back behavior.
- Keep the regular Details view single-pane unless a later user decision asks
  for a split Details-plus-editor layout.

### 6. Validate and Release

- Add Android unit tests for view-mode persistence and tree row state.
- Add shared/controller tests only if a required existing file API contract
  changes; no protocol change is expected.
- Manually test remote and actual Termux-host projects on an Android emulator
  and an ARM64 device.
- Rebuild both Android ABIs and upload fresh APKs to `tmpfiles.org` after each
  mobile source change, with verified SHA-256 hashes.

## Acceptance Criteria

1. Users can switch between Details and Explorer without losing the current
   project, file selection, editor content, or unrelated screen state.
2. Explorer is entirely native Android UI and uses no server-rendered web page
   or WebView-based code editor.
3. Folder expansion is lazy, cancellable, and renders only visible rows.
4. Existing file operations behave identically in both views.
5. Seti file icons, selected-file state, context actions, and keyboard
   navigation are visible and accessible.
6. A local-Termux project works through the existing authenticated localhost
   file API without exposing Termux private files to the Android process.
7. Explorer refreshes only relevant tree data; it does not refresh all app
   pages or disrupt an active chat/session.
8. Narrow devices remain usable; wide devices gain a native two-pane explorer
   and editor layout.

## Research Basis

- **Squircle CE** is the primary behavior reference: a maintained Apache-2.0
  native Android editor with a flattened lazy Explorer tree. Reuse concepts,
  not its UI code or Compose stack:
  <https://github.com/massivemadness/Squircle-CE>
- **AndroidIDE** demonstrates a native Android file tree, but is archived and
  GPL-3.0; do not copy code from it:
  <https://github.com/AndroidIDEOfficial/AndroidIDE>
- **Acode** is a strong mobile editor but is Cordova/WebView-based, so it does
  not meet this native-UI requirement:
  <https://github.com/Acode-Foundation/Acode>
- Android's native `RecyclerView`, `SlidingPaneLayout`, and Storage Access
  Framework documentation:
  <https://developer.android.com/develop/ui/views/layout/recyclerview>
  <https://developer.android.com/develop/ui/views/layout/twopane>
  <https://developer.android.com/training/data-storage/shared/documents-files>
