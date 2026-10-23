### Atelier — English. Reference language: any key missing from another
### translation is shown in English.
###
### Every locales/*.ftl file has the same keys, in the same order, so they can
### be compared side by side. Syntax: https://projectfluent.org/fluent/guide/

## Selection tool options

options-select-add-hint = Add to selection
options-select-subtract-hint = Subtract from selection
options-select-intersect-hint = Intersect with selection
options-tolerance = Tolerance
options-contiguous = Contiguous
options-sample-all-layers = Sample All Layers
options-lasso-width = Width
options-lasso-width-hint = Distance within which the path looks for an edge.
options-lasso-frequency = Frequency
options-lasso-frequency-hint = The higher it is, the closer together the anchor points.
options-position = Position
options-size = Size
options-selection-inverted-hint = Inverted: everything except this area.
options-selection-bounds-hint = Size of the box that contains the selection.
options-no-selection = No selection.
options-select-subject = Select Subject
options-remove-background = Remove Background

## Paint Bucket tool options

options-bucket-hint = Fills with the foreground color
options-bucket-contiguous-tip = Unchecked: also fills areas of the same color that don't touch the click.

## Type tool options

options-text-locked = Type layer locked
options-text-locked-hint = Unlock it in the Layers panel to edit it, or click elsewhere to create a type layer
options-font-default = Sans-serif (default)
options-font = Font
options-font-search = Search…
options-align-left = Left align text
options-align-center = Center text
options-align-right = Right align text
options-color = Color
options-foreground = Foreground
options-use-foreground-tip = Use the foreground color
options-text-color-hint = New type layers take the foreground color

## Menu bar

menu-file = File
menu-file-open = Open…
menu-file-save = Save
menu-file-save-as = Save As…
menu-file-quit = Exit
menu-edit = Edit
menu-edit-undo = Undo
menu-edit-redo = Redo
menu-edit-cut = Cut
menu-edit-copy = Copy
menu-edit-paste = Paste
menu-edit-clear = Clear
menu-edit-fill-foreground = Fill with Foreground Color
menu-edit-fill-background = Fill with Background Color
menu-edit-fill = Fill…
menu-edit-free-transform = Free Transform
menu-image = Image
menu-image-adjustments = Adjustments
menu-image-image-size = Image Size…
menu-image-canvas-size = Canvas Size…
menu-image-rotation = Image Rotation
menu-image-rotate-180 = 180°
menu-image-rotate-cw = 90° Clockwise
menu-image-rotate-ccw = 90° Counter Clockwise
menu-image-rotate-arbitrary = Arbitrary…
menu-image-flip-horizontal = Flip Canvas Horizontal
menu-image-flip-vertical = Flip Canvas Vertical
menu-image-crop = Crop
menu-image-crop-to-element = Crop to Selected Element
menu-image-trim = Trim (All Visible Content)
menu-layer = Layer
menu-layer-new = New Layer
menu-layer-via-copy = Layer via Copy
menu-layer-duplicate = Duplicate Layer
menu-layer-delete = Delete Layer
menu-layer-arrange = Arrange
menu-layer-bring-to-front = Bring to Front
menu-layer-bring-forward = Bring Forward
menu-layer-send-backward = Send Backward
menu-layer-send-to-back = Send to Back
menu-layer-toggle-lock = Lock / Unlock
menu-layer-merge-down = Merge Down
menu-layer-merge-visible = Merge Visible
menu-layer-flatten = Flatten Image
menu-layer-remove-background = Remove Background
menu-layer-make-transparent = Make a Color Transparent…
menu-layer-split = Split into Elements…
menu-select = Select
menu-select-all = All
menu-select-deselect = Deselect
menu-select-reselect = Reselect
menu-select-inverse = Inverse
menu-select-subject = Subject
menu-view = View
menu-view-zoom-in = Zoom In
menu-view-zoom-out = Zoom Out
menu-view-fit-on-screen = Fit on Screen

## Colors

panel-foreground = Foreground
panel-foreground-tip = Brush, text, fill
panel-background = Background
panel-background-tip = Background fill
panel-swap-colors = Swap
panel-default-colors = Black / White

## Brush tool options

options-brush-hint = Paints with the foreground color
options-hardness = Hardness
options-opacity = Opacity

## Crop tool options

options-canvas-origin = Origin { $x }, { $y }
options-fit-visible-pixels = Fit to Visible Pixels
options-fit-visible-pixels-tip = Frames exactly the content of all visible layers.

## Move tool options

options-layer-locked = Layer locked
options-layer-locked-hint = Click another element, or unlock this one in the Layers panel
options-center-on-canvas = Center on Canvas
options-rotation = Rotation
options-constrain-proportions = Constrain Proportions
options-fit-canvas = Fit Canvas
options-fit-canvas-tip =
    Checked: the canvas takes exactly the size and place of the element (handy for resizing a single image).
    Unchecked: the canvas does not change.

## Layers panel

panel-layers = Layers
panel-delete-layer-tip = Delete layer
panel-new-layer-tip = New layer
panel-no-image = No image.
panel-unlock-tip = Unlock

## Layer properties

panel-properties = Properties
panel-name = Name
panel-opacity = Opacity
panel-blend-mode = Blend Mode
panel-size = Size
panel-align-left-tip = Left
panel-align-center-h-tip = Center
panel-align-right-tip = Right
panel-align-top-tip = Top
panel-align-center-v-tip = Middle
panel-align-bottom-tip = Bottom
panel-distribute-h-tip = Distribute horizontally (3 layers or more)
panel-distribute-v-tip = Distribute vertically (3 layers or more)
panel-effects = Effects
panel-stroke = Stroke
panel-drop-shadow = Drop Shadow
panel-fill-foreground = Foreground
panel-fill-background = Background

## Shared option fields

options-width = Width
options-height = Height
options-apply = Apply

## Language menu

menu-edit-language = Language

## Tools

tool-transform = Move
tool-brush = Brush
tool-eraser = Eraser
tool-text = Horizontal Type
tool-canvas = Crop
tool-bucket = Paint Bucket
tool-eyedropper = Eyedropper
tool-select = Rectangular Marquee
tool-lasso = Magnetic Lasso
tool-magic-wand = Magic Wand
tool-hand = Hand
tool-zoom = Zoom

## Resampling methods

app-resample-nearest = Nearest Neighbor (pixel art)
app-resample-bilinear = Bilinear
app-resample-bicubic = Bicubic
app-resample-lanczos = Lanczos (best quality)

## Fill dialog contents

app-fill-foreground = Foreground Color
app-fill-background = Background Color
app-fill-color = Color…
app-fill-black = Black
app-fill-gray = 50% Gray
app-fill-white = White

## Status messages, file dialogs and default contents

status-opened = Opened: { $path }
status-open-failed = Could not open { $path }: { $error }
status-import-failed = Could not import { $path }: { $error }
app-file-filter-all = All Files
app-dialog-save-as = Save As
status-saved = Saved: { $path }
status-nothing-to-copy = Nothing to copy: the layer is empty.
status-copied = "{ $name }" copied
status-copy-failed = Could not copy: { $error }
status-clipboard-no-image = The clipboard does not contain an image.
app-pasted-layer-name = Pasted
status-pasted = Image pasted ({ $width } × { $height } px)
status-no-color = No color here (transparent).
status-background-color = Background color: { $hex }
status-foreground-color = Foreground color: { $hex }
status-image-size = Image size: { $width } × { $height } px
status-filled = "{ $name }" filled with { $content }
status-no-visible-pixel = This layer has no visible pixels on the canvas.
status-layer-is-locked = "{ $name }" is locked: unlock it in the Layers panel.
status-selection-empty = The selection is empty on this layer.
status-layer-locked = "{ $name }" locked
status-layer-unlocked = "{ $name }" unlocked
app-dialog-open-image = Open Image
status-element-fills-canvas = "{ $name }" already fills the whole canvas.
status-content-fills-canvas = The content already fills the whole canvas.
status-nothing-to-crop = Nothing visible to crop.
status-layers-merged = Layers merged into "{ $name }"
status-merge-down-failed = Cannot merge: there must be a visible, unlocked layer below.
status-merge-visible-failed = At least two visible layers are needed to merge.
status-rotated = , rotated { $angle }°
status-canvas-fit-element = Canvas fitted to the element: { $width } × { $height } px{ $rotated }
status-element-placed = Element: { $width } × { $height } px at ({ $x }, { $y }){ $rotated }
status-hardness = Hardness: { $value }%
status-brush-size = Size: { $value } px
status-tool-opacity = Tool opacity: { $percent }%
status-layer-opacity = "{ $name }" opacity: { $percent }%

## Split into elements

dialog-split-empty = "{ $name }" is empty: nothing to split.
dialog-split-single = "{ $name }" contains only one element at this gap.
dialog-split-done = "{ $name }" split into { $count } layers.
dialog-split-title = Split into Elements
dialog-split-layer = Layer
dialog-split-gap = Max gap within an element
dialog-split-gap-hint = Increase to group the letters of a word, decrease to separate nearby elements.
dialog-split-no-pixels = No visible pixels.
dialog-split-found = { $count ->
    [one] Only 1 element found.
   *[other] { $count } elements found.
}
dialog-split-apply = Split
dialog-cancel = Cancel

## Make a color transparent

dialog-transparency-title = Make a Color Transparent
dialog-transparency-color = Color to remove
dialog-transparency-color-tip = Or click the image
dialog-transparency-hint = Tip: click the image to pick it.
dialog-transparency-tolerance = Tolerance
dialog-transparency-softness = Softness
dialog-apply = Apply
dialog-transparency-done = Color { $color } made transparent

## Image adjustments

dialog-adjust-tone-balance = Tone Balance
dialog-adjust-shadows = Shadows
dialog-adjust-midtones = Midtones
dialog-adjust-highlights = Highlights
dialog-color-cyan = Cyan
dialog-color-red = Red
dialog-color-magenta = Magenta
dialog-color-green = Green
dialog-color-yellow = Yellow
dialog-color-blue = Blue
dialog-adjust-preserve-luminosity = Preserve Luminosity
dialog-adjust-brightness = Brightness
dialog-adjust-contrast = Contrast
dialog-adjust-channel = Channel
dialog-adjust-auto = Auto
dialog-adjust-auto-tip = Stretches each channel, ignoring 0.1% of the pixels at each end
dialog-adjust-vibrance = Vibrance
dialog-adjust-saturation = Saturation
dialog-vibrance-hint = Vibrance mostly boosts dull colors and spares those that are already vivid (skin tones included).
dialog-adjust-hue = Hue
dialog-adjust-lightness = Lightness
dialog-adjust-colorize = Colorize
dialog-adjust-colorize-tip = A single hue for the whole layer
dialog-adjust-reset = Reset
dialog-ok = OK
dialog-preview = Preview
dialog-close-tip = Close
dialog-adjust-applied = { $name } applied

## Fill

dialog-fill-title = Fill
dialog-fill-contents = Contents
dialog-fill-color-tip = Fill color
dialog-fill-opacity = Opacity
dialog-fill-preserve-transparency = Preserve Transparency
dialog-fill-preserve-transparency-tip = Pixels keep their opacity: empty stays empty
dialog-fill-with-foreground = the foreground color
dialog-fill-with-background = the background color
dialog-fill-with-color = color { $color }
dialog-fill-with-black = black
dialog-fill-with-gray = 50% gray
dialog-fill-with-white = white

## Image size

dialog-image-size-title = Image Size
dialog-image-size-current = Current
dialog-image-size-constrain = Constrain Proportions
dialog-image-size-resample = Resample
dialog-image-size-hint = All layers follow; text stays editable.

## AI model download

dialog-model-hint = No image is sent online. Without AI, detection only works when the subject stands out clearly from a simple background.
dialog-model-download = Download
dialog-model-no-ai = Without AI

## Unsaved changes

dialog-confirm-title = Unsaved Changes
dialog-confirm-text = Save changes to "{ $name }"?
dialog-confirm-hint = If you don't, your changes will be lost.
dialog-confirm-save = Save
dialog-confirm-dont-save = Don't Save

## Levels and curves editors

dialog-channel-rgb = RGB
dialog-levels-input = Input Levels
dialog-levels-output = Output Levels
dialog-curves-no-point = No point selected
dialog-curves-input = Input
dialog-curves-output = Output

## Selection tools

select-wand-outside-layer = Click on the pixels of the active layer.
select-ai-unavailable = AI unavailable: { $error }; non-AI method used.
select-subject-searching = Finding subject…
select-subject-failed = The subject search failed.
select-subject-not-found = No clear subject found: try the Magnetic Lasso or the Magic Wand.
select-layer-changed = The layer changed during the search.
select-background-removed = Background of "{ $name }" removed
select-subject-selected-note = Subject selected — { $note }
select-subject-selected = Subject selected

## Canvas context menu

canvas-menu-crop-to-selection = Crop to Selection
canvas-menu-crop-to-element = Crop to Element
canvas-menu-crop-to-content = Crop to All Content
canvas-menu-center = Center on Canvas
canvas-menu-bring-to-front = Bring to Front
canvas-menu-send-to-back = Send to Back
canvas-menu-duplicate = Duplicate
canvas-menu-delete-layer = Delete Layer
canvas-menu-unlock = Unlock
canvas-menu-lock = Lock
canvas-menu-make-transparent = Make a Color Transparent…
canvas-menu-remove-background = Remove Background
canvas-menu-select-subject = Select Subject
canvas-menu-adjustments = Adjustments

## Canvas status messages

canvas-status-text-rasterized = "{ $name }" rasterized (no longer editable as text)
canvas-status-layer-moved = "{ $name }" moved to ({ $x }, { $y })

## Document: default names

doc-background-layer-name = Background
doc-untitled = Untitled
doc-image-layer-default-name = Image
doc-layer-default-name = Layer { $number }
doc-layer-copy-name = { $name } copy
doc-layer-selection-name = { $name } (selection)
doc-text-layer-default-name = Text

## Blend modes

blend-normal = Normal
blend-multiply = Multiply
blend-screen = Screen

## Image adjustments (Image > Adjustments menu, adjustment window titles)

adjust-brightness-contrast = Brightness/Contrast
adjust-levels = Levels
adjust-curves = Curves
adjust-vibrance = Vibrance
adjust-hue-saturation = Hue/Saturation
adjust-color-balance = Color Balance

## AI Select Subject errors

ai-error-model-corrupt = model file incomplete or altered
ai-error-download-cancelled = Download cancelled
ai-error-download-interrupted = the download stopped unexpectedly
ai-model-subject-purpose = Select Subject, Remove Background
ai-model-objects-purpose = Object Selection tool
ai-error-no-localappdata = LOCALAPPDATA is not set
ai-error-dll-not-loaded = { $name } could not be loaded

## File menu (redesign)

menu-file-close = Close

## Title bar: window buttons

title-minimize = Minimize
title-maximize = Maximize
title-restore = Restore Down
title-close = Close

## Command palette

palette-placeholder = Search commands, tools…
palette-suggested = Suggested
palette-tools = Tools
palette-commands = Commands
palette-tool = Tool
palette-no-match = No command matches "{ $query }".
palette-navigate = Navigate
palette-run = Run
palette-close = Close

## Options bar

optbar-no-document = Open an image to use this tool.

## Options bar: Move tool

optbar-width-short = W
optbar-height-short = H
optbar-rotate-ccw = Rotate 90° Counter Clockwise
optbar-rotate-cw = Rotate 90° Clockwise
optbar-scale = Scale

## Options bar: selection tools

optbar-select-new = New selection

## Options bar: Eraser tool

optbar-eraser-hint = Erases to transparency on the active layer

## Options bar: Eyedropper tool

optbar-background = Background

## Options bar: Type tool

optbar-use-foreground = Use Foreground

## Side panel: Properties

side-kind-image = Image
side-kind-text = Type
side-kind-locked = { $kind } · locked
side-locked-notice = Layer locked — unlock it in the Layers panel.
side-transform = Transform
side-rotate-left-tip = Rotate 90° Counter Clockwise
side-rotate-right-tip = Rotate 90° Clockwise
side-align-to-canvas = Align to canvas
side-align-layers = Align { $count } layers
side-layer = Layer
side-character = Character
side-text-color-tip = Text color
side-fill-layer = Fill

## Side panel: Layers

side-layer-count = { $count ->
    [one] { $count } layer
   *[other] { $count } layers
}
side-lock-tip = Lock / unlock

## Document tabs

doc-tab-close = Close

## Zoom bar

zoombar-actual-pixels = 100%

## Start screen (no image open)

welcome-title = Open an image
welcome-subtitle = Drop a file anywhere in the window, or pick one.
welcome-recent = Recent
welcome-forget-recent = Remove from List

## Status bar: keys of the active tool

hint-key-shift = Shift
hint-key-enter = Enter
hint-key-esc = Esc
hint-key-space = Space
hint-key-backspace = Backspace
hint-key-click = Click
hint-key-drag = Drag

## Status bar: what the keys do

hint-apply = apply
hint-cancel = cancel
hint-move-shift = 15° · free proportions
hint-from-center = from center
hint-no-snap = no snapping
hint-layer-opacity = layer opacity
hint-add = add
hint-subtract = subtract
hint-deselect = deselect
hint-inverse = inverse
hint-layer-via-copy = layer via copy
hint-anchor-point = anchor point
hint-remove-point = remove point
hint-close-path = close
hint-size = size
hint-hardness = hardness
hint-opacity = opacity
hint-straight-line = straight line
hint-pick-color = pick a color
hint-pan = pan
hint-fill = fill
hint-foreground = foreground
hint-background = background
hint-keep-ratio = keep proportions
hint-zoom-in = zoom in
hint-zoom-out = zoom out
hint-hand-any-tool = hand with any tool
hint-zoom = zoom
hint-fit = fit on screen

## Canvas Size, Image Rotation

dialog-canvas-size-title = Canvas Size
dialog-canvas-size-current = Current
dialog-canvas-size-new = New Size
dialog-canvas-size-relative = Relative
dialog-canvas-size-anchor = Anchor
dialog-canvas-size-extension = Canvas Extension Color
dialog-canvas-size-transparent = Transparent
dialog-rotate-title = Rotate Canvas
dialog-rotate-angle = Angle
dialog-rotate-cw = °CW
dialog-rotate-ccw = °CCW
status-image-rotated = Image rotated { $angle }°
status-canvas-resized = Canvas: { $width } × { $height } px

## AI Models window

models-menu = AI Models…
models-title = AI Models
models-hint = AI models run on this computer: no image is sent online.
models-state-downloaded = Downloaded
models-state-downloading = Downloading…
models-state-incomplete = Incomplete
models-state-not-downloaded = Not downloaded
models-download-size = Download
models-on-disk = On disk
models-runs-on = Runs on
models-license = License
models-device-gpu = GPU (DirectML)
models-device-cpu = CPU
models-device-gpu-if-available = GPU if available
models-delete = Delete
models-in-use = In use
models-total = Disk space used: { $size }
models-open-folder = Open Folder
models-open-folder-failed = Could not open the models folder ({ $error }).
models-size-mb = { $size } MB
models-size-gb = { $size } GB

## Deleting a model

models-delete-title = Delete Model
models-delete-text = Delete { $name } from this computer? This frees { $size }.
models-delete-hint = Used by: { $feature }. It will be offered for download again the next time you use it.

## Model download

models-prompt-title = Download AI Model
models-prompt-text = "{ $feature }" uses an AI model, { $name }, that runs on this computer. It needs to be downloaded once: { $size }.
models-prompt-hint = No image is sent online. You can delete the model at any time in Edit › AI Models.
models-status-downloading = Downloading { $name }… { $percent }%
models-status-downloaded = { $name } downloaded.
models-status-failed = Could not download { $name } ({ $error }).
models-status-deleted = { $name } deleted.
models-status-delete-failed = Could not delete { $name } ({ $error }).

## Object Selection tool

objsel-tool = Object Selection
objsel-hint = Click an object, or drag a rectangle around it.
objsel-analysing = Analyzing the image…
objsel-downloading = Downloading the Object Selection model… { $percent }%
objsel-failed = Object Selection failed ({ $error })
objsel-not-found = No object found there.

## Object Selection tool: status bar hints

objsel-hint-object = select the object
objsel-hint-rectangle = object in the rectangle
objsel-hint-intersect = intersect

## File menu: Export As

file-menu-export-as = Export As…
file-export-title = Export As
file-export-format = Format
file-export-quality = Quality
file-export-ok = Export
file-export-jpeg-hint = JPEG has no transparency: transparent areas become white.
file-export-lossless-hint = Lossless: every pixel is kept, transparency included.

## File dialogs

file-filter-supported = All Supported Files
file-filter-atelier = Atelier Document

## File status messages

file-status-opened-first-frame = Opened: { $path } (first frame only: animations are not supported)
file-status-imported-first-frame = Layer added: { $path } (first frame only: animations are not supported)
file-status-open-in-another-tab = "{ $name }" is open in another tab: close it first, or choose another name.
file-status-save-failed = Could not save "{ $name }": { $error }. Nothing was overwritten.
file-status-exported = Exported: { $path }
file-status-export-failed = Could not export "{ $name }": { $error }. Nothing was overwritten.
file-status-paste-failed = Could not paste the image: { $error }

## File errors (after "Could not open …:")

file-error-not-found = file not found
file-error-permission = permission denied
file-error-disk-full = not enough disk space
file-error-damaged = the file is damaged or incomplete
file-error-unsupported = unsupported file format
file-error-too-large = the image is too large ({ $width } × { $height } px; at most { $max } px per side)
file-error-empty = the image is empty (0 pixels)
file-error-not-atelier = not an Atelier document
file-error-newer-version = made by a newer version of Atelier (format { $version }): update Atelier to open it
file-error-format-limit = { $format } images are limited to { $max } px per side

## Layer merging

doc-merge-locked = Cannot merge: a layer is locked.
doc-merge-too-large = Cannot merge: the layers are too far apart.

## Flatten Image

doc-discard-hidden = Discard hidden layers?

## Selection and Move tools: refusals

select-layer-hidden = "{ $name }" is hidden: show it to move its pixels.
select-element-too-large = The element would be larger than { $max } px: transform not applied.

## AI errors and devices (fix-ai)
ai-error-internal = internal error
ai-error-stalled = the connection stopped responding
ai-device-gpu-and-cpu = GPU (DirectML) and CPU

## Numbers

number-decimal-separator = .
number-percent = { $value }%

## Key names (menus, tooltips, palette, status bar)

key-delete = Delete
key-wheel = Wheel

## Blend modes

core-blend-pass-through = Pass Through
core-blend-dissolve = Dissolve
core-blend-darken = Darken
core-blend-color-burn = Color Burn
core-blend-linear-burn = Linear Burn
core-blend-darker-color = Darker Color
core-blend-lighten = Lighten
core-blend-color-dodge = Color Dodge
core-blend-linear-dodge = Linear Dodge (Add)
core-blend-lighter-color = Lighter Color
core-blend-overlay = Overlay
core-blend-soft-light = Soft Light
core-blend-hard-light = Hard Light
core-blend-vivid-light = Vivid Light
core-blend-linear-light = Linear Light
core-blend-pin-light = Pin Light
core-blend-hard-mix = Hard Mix
core-blend-difference = Difference
core-blend-exclusion = Exclusion
core-blend-subtract = Subtract
core-blend-divide = Divide
core-blend-hue = Hue
core-blend-saturation = Saturation
core-blend-color = Color
core-blend-luminosity = Luminosity
core-status-blend = { $name }: { $mode }

## Adjustments (Image > Adjustments, adjustment layers)

core-adjust-black-white = Black & White
core-adjust-photo-filter = Photo Filter
core-adjust-invert = Invert
core-adjust-posterize = Posterize
core-adjust-threshold = Threshold
core-adjust-gradient-map = Gradient Map
core-adjust-no-settings = This adjustment has no settings.
core-bw-reds = Reds
core-bw-yellows = Yellows
core-bw-greens = Greens
core-bw-cyans = Cyans
core-bw-blues = Blues
core-bw-magentas = Magentas
core-bw-tint = Tint
core-photo-filter-color = Color
core-photo-filter-density = Density
core-posterize-levels = Levels
core-threshold-level = Threshold Level

## Gradients (fill layers, Gradient Overlay, Gradient Map)

core-gradient-reverse = Reverse
core-gradient-style = Style
core-gradient-angle-tip = Angle
core-gradient-scale = Scale

## Layer kinds and default names

core-kind-group = Group
core-kind-fill = Fill
core-group-name = Group { $number }
core-layer-numbered-name = { $name } { $number }
core-solid-fill-name = Color Fill { $number }
core-gradient-fill-name = Gradient Fill { $number }
core-solid-color = Solid Color
core-gradient = Gradient

## Properties panel: layer, fill layer, mask, styles

core-fill-opacity = Fill
core-fill-opacity-tip = Fill opacity: like the opacity, but the layer styles keep theirs
core-fill-content = Fill
core-fill-color = Color
core-mask = Layer Mask
core-mask-density = Density
core-mask-feather = Feather
core-mask-load-selection = Load Selection from Mask
core-mask-invert = Invert
core-mask-apply = Apply Mask
core-mask-delete = Delete Mask
core-mask-enable = Enable Layer Mask
core-mask-disable = Disable Layer Mask
core-mask-link = Link
core-mask-unlink = Unlink
core-mask-reveal-all = Reveal All
core-mask-hide-all = Hide All
core-mask-reveal-selection = Reveal Selection
core-mask-hide-selection = Hide Selection
core-style-open-tip = Settings in the Layer Style window

## Layers panel

core-group-fold-tip = Open / close the group
core-link-layers-tip = Link layers
core-unlink-layers-tip = Unlink layers
core-styles-tip = Add a layer style
core-new-adjustment-tip = New fill or adjustment layer
core-new-group-tip = New group

## Layer menu

core-menu-new-group = New Group
core-menu-layer-style = Layer Style
core-menu-new-fill-layer = New Fill Layer
core-menu-new-adjustment-layer = New Adjustment Layer
core-menu-layer-mask = Layer Mask
core-menu-create-clipping = Create Clipping Mask
core-menu-release-clipping = Release Clipping Mask
core-menu-group-layers = Group Layers
core-menu-ungroup-layers = Ungroup Layers
core-menu-merge-group = Merge Group

## Layer Style window

core-layer-style = Layer Style
core-blending-options = Blending Options
core-clear-layer-style = Clear Layer Style
core-style-bevel = Bevel & Emboss
core-style-inner-shadow = Inner Shadow
core-style-inner-glow = Inner Glow
core-style-color-overlay = Color Overlay
core-style-gradient-overlay = Gradient Overlay
core-style-outer-glow = Outer Glow
core-style-size = Size
core-style-angle = Angle
core-style-color-tip = Color
core-bevel-direction = Direction
core-bevel-up = Up
core-bevel-down = Down
core-bevel-depth = Depth
core-bevel-soften = Soften
core-bevel-altitude = Altitude
core-bevel-highlight = Highlight
core-bevel-shadow = Shadow
core-glow-spread = Spread
core-glow-choke = Choke

## Filter menu

filter-menu = Filter
filter-last = Last Filter
filter-group-blur = Blur
filter-group-noise = Noise
filter-group-pixelate = Pixelate
filter-group-render = Render
filter-group-sharpen = Sharpen
filter-group-stylize = Stylize
filter-group-other = Other

## Filters (menu items and dialog titles)

filter-box-blur = Box Blur
filter-gaussian-blur = Gaussian Blur
filter-motion-blur = Motion Blur
filter-surface-blur = Surface Blur
filter-add-noise = Add Noise
filter-dust-and-scratches = Dust & Scratches
filter-median = Median
filter-reduce-noise = Reduce Noise
filter-mosaic = Mosaic
filter-clouds = Clouds
filter-smart-sharpen = Smart Sharpen
filter-unsharp-mask = Unsharp Mask
filter-emboss = Emboss
filter-find-edges = Find Edges
filter-high-pass = High Pass
filter-maximum = Maximum
filter-minimum = Minimum
filter-offset = Offset
filter-applied = { $name } applied
filter-no-pixels = This layer has no pixels to filter: choose a pixel layer, or its mask.

## Filter dialogs

filter-preview-tip = Drag to look elsewhere; hold the button down to see the original.
filter-radius = Radius
filter-amount = Amount
filter-sharpen-amount = Amount
filter-threshold = Threshold
filter-unit-levels = levels
filter-angle = Angle
filter-distance = Distance
filter-strength = Strength
filter-preserve-details = Preserve Details
filter-color-noise = Reduce Color Noise
filter-sharpen-details = Sharpen Details
filter-reduce-noise-amount = Reduce Noise
filter-remove = Remove
filter-lens-blur = Lens Blur
filter-shadows-highlights = Shadows / Highlights
filter-shadows = Shadows
filter-highlights = Highlights
filter-fade-amount = Fade Amount
filter-tonal-width = Tonal Width
filter-distribution = Distribution
filter-uniform = Uniform
filter-gaussian = Gaussian
filter-monochromatic = Monochromatic
filter-offset-horizontal = Horizontal (pixels right)
filter-offset-vertical = Vertical (pixels down)
filter-undefined-areas = Undefined Areas
filter-set-transparent = Set to Transparent
filter-repeat-edges = Repeat Edge Pixels
filter-wrap-around = Wrap Around
filter-preserve = Preserve
filter-squareness = Squareness
filter-roundness = Roundness
filter-height = Height
filter-cell-size = Cell Size
filter-unit-square = square

## Selection tools and their options

sel-tool-ellipse = Elliptical Marquee
sel-tool-lasso = Lasso
sel-tool-polygonal-lasso = Polygonal Lasso
sel-feather = Feather
sel-feather-tip = Softens the edge of the new selections.
sel-anti-alias = Anti-alias
sel-style = Style
sel-style-normal = Normal
sel-style-fixed-ratio = Fixed Ratio
sel-style-fixed-size = Fixed Size
sel-swap-size = Swap height and width
sel-hint-draw = draw
sel-hint-straight = straight segments
sel-hint-point = add a point
sel-hint-45 = 45° steps
sel-hint-freehand = freehand

## Select menu

sel-menu-color-range = Color Range…
sel-menu-select-and-mask = Select and Mask…
sel-menu-modify = Modify
sel-menu-border = Border…
sel-menu-smooth = Smooth…
sel-menu-expand = Expand…
sel-menu-contract = Contract…
sel-menu-feather = Feather…
sel-menu-grow = Grow
sel-menu-similar = Similar
sel-menu-transform = Transform Selection
sel-menu-quick-mask = Edit in Quick Mask Mode
sel-menu-load = Load Selection…
sel-menu-save = Save Selection…

## Select › Modify windows

sel-border-title = Border Selection
sel-border-width = Width
sel-smooth-title = Smooth Selection
sel-smooth-radius = Sample Radius
sel-expand-title = Expand Selection
sel-expand-by = Expand By
sel-contract-title = Contract Selection
sel-contract-by = Contract By
sel-feather-title = Feather Selection
sel-feather-radius = Feather Radius
sel-modify-at-bounds = Apply effect at canvas bounds
sel-modify-at-bounds-tip = Off, the selection goes on past the edges of the canvas.

## Save Selection, Load Selection

sel-save-title = Save Selection
sel-load-title = Load Selection
sel-channel = Channel
sel-channel-new = New
sel-channel-name = Name
sel-channel-default-name = Alpha { $n }
sel-operation = Operation
sel-op-new-channel = New Channel
sel-op-replace-channel = Replace Channel
sel-op-add-channel = Add to Channel
sel-op-subtract-channel = Subtract from Channel
sel-op-intersect-channel = Intersect with Channel
sel-op-new-selection = New Selection
sel-load-invert = Invert

## Color Range

sel-range-title = Color Range
sel-range-sampled = Sampled Colors
sel-range-eyedropper = Eyedropper
sel-range-add = Add to Sample
sel-range-subtract = Subtract from Sample
sel-range-fuzziness = Fuzziness
sel-range-show-selection = Selection
sel-range-show-image = Image
sel-range-preparing = Preparing the preview…
sel-range-preview = Selection Preview
sel-range-preview-none = None
sel-range-preview-grayscale = Grayscale
sel-range-preview-black = Black Matte
sel-range-preview-white = White Matte
sel-range-preview-quick-mask = Quick Mask
sel-range-invert = Invert
sel-range-hint = Click the image to sample a color: { $add } adds one, { $subtract } subtracts one.

## Select and Mask

sel-mask-title = Select and Mask
sel-mask-view = View Mode
sel-mask-view-onion = Onion Skin
sel-mask-view-black = On Black
sel-mask-view-white = On White
sel-mask-view-overlay = Overlay
sel-mask-view-bw = Black & White
sel-mask-transparency = Transparency
sel-mask-opacity = Opacity
sel-mask-global = Global Refinements
sel-mask-smooth = Smooth
sel-mask-feather = Feather
sel-mask-contrast = Contrast
sel-mask-shift-edge = Shift Edge
sel-mask-output = Output To
sel-mask-output-selection = Selection
sel-mask-output-layer-mask = Layer Mask
sel-mask-output-new-layer = New Layer
sel-mask-output-new-layer-mask = New Layer with Layer Mask
sel-mask-replaces-mask = The layer's mask is replaced.

## Transform Selection

sel-transform-hint = Drag the handles to scale, outside the frame to rotate

## Status messages

sel-status-working = Computing the selection…
sel-status-nothing-selected = No pixels are selected anymore.
sel-status-faint = No pixels are more than 50% selected: the selection edges won't be visible.
sel-status-saved = Selection saved in "{ $name }".
sel-status-quick-mask-on = Quick Mask: paint with black to mask, with white to select; { $key } to finish.
sel-status-quick-mask-off = Selection made from the Quick Mask.

## Painting tools: names

paint-tool-gradient = Gradient
paint-tool-dodge = Dodge
paint-tool-burn = Burn
paint-tool-sponge = Sponge
paint-tool-blur = Blur
paint-tool-sharpen = Sharpen
paint-tool-smudge = Smudge

## Brush preset picker and brush options

paint-brush-picker-tip = Brush: size, hardness, angle, roundness, spacing and presets
paint-angle = Angle
paint-roundness = Roundness
paint-spacing = Spacing
paint-spacing-tip = Distance between the dabs of the brush, as a percentage of its diameter
paint-preset-hard-round = Hard Round
paint-preset-soft-round = Soft Round
paint-preset-chalk = Chalk
paint-preset-spatter = Spatter
paint-preset-dry-brush = Dry Brush
paint-preset-charcoal = Charcoal
paint-flow = Flow
paint-smoothing = Smoothing
paint-airbrush-tip = Enable airbrush-style build-up effects
paint-pressure-opacity-tip = Always use pressure for opacity (pen tablet)
paint-pressure-size-tip = Always use pressure for size (pen tablet)
paint-mode = Mode
paint-range = Range
paint-range-shadows = Shadows
paint-range-midtones = Midtones
paint-range-highlights = Highlights
paint-exposure = Exposure
paint-protect-tones = Protect Tones
paint-protect-tones-tip = Minimizes clipping in the shadows and highlights, and keeps colors from shifting hue
paint-sponge-desaturate = Desaturate
paint-sponge-saturate = Saturate
paint-vibrance = Vibrance
paint-vibrance-tip = Minimizes clipping for fully saturated colors
paint-strength = Strength
paint-sample-all-tip = Works on what all the visible layers show, and paints the result on the active layer
paint-finger-painting = Finger Painting
paint-finger-painting-tip = Starts each stroke with the foreground color
paint-dodge-hint = Lightens the areas you paint
paint-burn-hint = Darkens the areas you paint
paint-sponge-hint = Changes the color saturation of the areas you paint
paint-blur-hint = Softens hard edges in the areas you paint
paint-sharpen-hint = Increases the contrast of edges in the areas you paint
paint-smudge-hint = Drags the color along the stroke, like a finger in wet paint

## Gradient tool options

paint-gradient-edit-tip = Click to edit the gradient
paint-gradient-presets = Gradient presets
paint-gradient-linear = Linear Gradient
paint-gradient-radial = Radial Gradient
paint-gradient-angle = Angle Gradient
paint-gradient-reflected = Reflected Gradient
paint-gradient-diamond = Diamond Gradient
paint-gradient-reverse = Reverse
paint-gradient-dither = Dither
paint-gradient-dither-tip = Adds a little noise to avoid banding
paint-gradient-transparency = Transparency
paint-gradient-transparency-tip = Uses the gradient's opacity stops
paint-gradient-hint = Drag across the image to draw the gradient
paint-gradient-fg-to-bg = Foreground to Background
paint-gradient-fg-to-transparent = Foreground to Transparent
paint-gradient-black-white = Black, White
paint-gradient-spectrum = Spectrum
paint-gradient-violet-orange = Violet, Orange
paint-gradient-copper = Copper

## Gradient Editor

paint-gradient-editor-title = Gradient Editor
paint-gradient-stops = Stops
paint-gradient-location = Location
paint-gradient-opacity = Opacity
paint-gradient-foreground = Foreground
paint-gradient-background = Background
paint-gradient-custom-color = Color
paint-gradient-stop-color = Stop color
paint-gradient-delete-stop = Delete

## Painting tools: status bar hints and messages

paint-hint-exposure = exposure
paint-hint-flow = flow
paint-hint-strength = strength
paint-hint-dodge = dodge
paint-hint-burn = burn
paint-hint-blur = blur
paint-hint-sharpen = sharpen
paint-hint-finger-painting = finger painting
paint-hint-gradient = draw the gradient
paint-hint-45 = 45° steps
paint-status-flow = Flow: { $percent }%
paint-status-exposure = Exposure: { $percent }%
paint-status-strength = Strength: { $percent }%
paint-status-nothing-to-paint = This layer has no pixels to paint on: add a layer mask, or choose a pixel layer.

## Color Picker

uip-picker-title = Color Picker
uip-picker-foreground-title = Color Picker (Foreground Color)
uip-picker-background-title = Color Picker (Background Color)
uip-picker-new = new
uip-picker-current = current
uip-picker-current-tip = Click to go back to the current color
uip-picker-add-to-swatches = Add to Swatches
uip-picker-h = H
uip-picker-s = S
uip-picker-b = B
uip-picker-red = R
uip-picker-green = G
uip-picker-blue = B
uip-picker-sample-hint = Click the image to sample a color.
uip-status-swatch-added = Added to Swatches: { $hex }

## Swatches panel

uip-swatches-title = Swatches
uip-swatches-recent = Recently used colors
uip-swatches-new-tip = Create new swatch from the foreground color
uip-swatches-hint = Click: foreground · { $background }: background · { $delete }: delete

## View, Window and Edit menus

uip-menu-show = Show
uip-menu-grid = Grid
uip-menu-guides = Guides
uip-menu-rulers = Rulers
uip-menu-snap = Snap
uip-menu-snap-to = Snap To
uip-menu-snap-layers = Layers
uip-menu-snap-bounds = Document Bounds
uip-menu-lock-guides = Lock Guides
uip-menu-clear-guides = Clear Guides
uip-menu-new-guide = New Guide…
uip-menu-window = Window
uip-menu-swatches = Swatches
uip-menu-preferences = Preferences
uip-menu-guides-grid = Guides & Grid…

## New Guide dialog

uip-new-guide-title = New Guide
uip-new-guide-orientation = Orientation
uip-new-guide-horizontal = Horizontal
uip-new-guide-vertical = Vertical
uip-new-guide-position = Position

## Guides & Grid preferences

uip-preferences-title = Guides & Grid
uip-preferences-grid = Grid
uip-preferences-gridline-every = Gridline Every
uip-preferences-subdivisions = Subdivisions

## Status bar

uip-status-rgb = R { $r }  G { $g }  B { $b }

## Retouching tools: names

retouch-tool-clone-stamp = Clone Stamp
retouch-tool-spot-healing = Spot Healing Brush
retouch-tool-healing = Healing Brush
retouch-tool-patch = Patch
retouch-tool-red-eye = Red Eye

## Retouching tools: options bar

retouch-aligned = Aligned
retouch-aligned-tip = Keep the same offset to the source from one stroke to the next; unchecked, each stroke starts again from the source point
retouch-sample = Sample
retouch-sample-current = Current Layer
retouch-sample-below = Current & Below
retouch-sample-all = All Layers
retouch-type = Type
retouch-content-aware = Content-Aware
retouch-content-aware-tip = Fill the area with details from around it
retouch-proximity-match = Proximity Match
retouch-proximity-match-tip = Fill the area with the pixels beside it that match its edge best
retouch-spot-sample-all-tip = Fill from what all the visible layers show
retouch-patch = Patch
retouch-patch-source = Source
retouch-patch-source-tip = Drag the selection to the area to patch it with
retouch-patch-destination = Destination
retouch-patch-destination-tip = Drag the selection to the area to patch with it
retouch-pupil-size = Pupil Size
retouch-darken-amount = Darken Amount
retouch-spot-hint = Paint over a blemish to remove it
retouch-patch-hint = Select an area, then drag it
retouch-red-eye-hint = Click a red pupil

## Retouching tools: status bar hints and messages

retouch-hint-source = set the source
retouch-hint-spot = remove a blemish
retouch-key-drag-inside = Drag inside
retouch-hint-patch = patch
retouch-hint-red-eye = remove a red eye
retouch-status-source-set = Source set
retouch-status-no-source = { $keys } to set the source first.
retouch-status-nothing-to-sample = Nothing around to sample from.
retouch-status-no-red-eye = No red eye found there.
retouch-status-select-first = Select the area to fill first.
retouch-needs-selection = Needs a selection
retouch-status-cancelled = { $name } cancelled
retouch-status-ai-failed = The AI model failed ({ $error }): filled without it.

## Content-Aware Fill

retouch-menu-content-aware-fill = Content-Aware Fill…
retouch-content-aware-fill = Content-Aware Fill
retouch-sampling-area = Sampling Area
retouch-sampling-auto = Auto
retouch-sampling-layer = Entire Layer
retouch-color-adaptation = Color Adaptation
retouch-color-adaptation-tip = Blend the fill with the colors and lighting around the selection
retouch-output-to = Output To
retouch-output-current = Current Layer
retouch-output-new = New Layer
retouch-use-ai = Use the AI model (LaMa)
retouch-use-ai-download = Use the AI model (LaMa, { $size } to download)
retouch-use-ai-hint = The AI model paints in large areas and complex scenes; without it, the fill is made of patches taken from the image.
retouch-model-purpose = Content-Aware Fill, Spot Healing Brush
retouch-model-hint = No image is sent online. Without AI, the fill is made of patches taken from the image.

## Edit › Transform: menu and history

xform-menu = Transform
xform-again = Again
xform-scale = Scale
xform-rotate = Rotate
xform-skew = Skew
xform-distort = Distort
xform-perspective = Perspective
xform-warp = Warp
xform-rotate-180 = Rotate 180°
xform-flip-horizontal = Flip Horizontal
xform-flip-vertical = Flip Vertical
xform-history-again = Transform Again

## Free Transform options bar: reference point, warp

xform-reference = Reference point location
xform-toggle-warp = Switch between free transform and warp modes
xform-horizontal = Horizontal
xform-vertical = Vertical
xform-bend = Bend
xform-style-custom = Custom
xform-style-arc = Arc
xform-style-arch = Arch
xform-style-bulge = Bulge
xform-style-flag = Flag
xform-style-wave = Wave

## Free Transform status bar hints

xform-hint-handle = Handle
xform-hint-distort = distort
xform-hint-skew = skew
xform-hint-perspective = perspective

## Free Transform status messages

xform-status-done = { $what }: "{ $name }"
xform-status-nothing = Nothing to transform on "{ $name }".
xform-status-empty = The selected area is empty.
xform-status-no-again = No transform to repeat.

## Vector tools: names, and the names of the shape layers

vector-tool-pen = Pen
vector-tool-freeform-pen = Freeform Pen
vector-tool-add-anchor = Add Anchor Point
vector-tool-delete-anchor = Delete Anchor Point
vector-tool-convert-point = Convert Point
vector-tool-path-selection = Path Selection
vector-tool-direct-selection = Direct Selection
vector-tool-rectangle = Rectangle
vector-tool-ellipse = Ellipse
vector-tool-triangle = Triangle
vector-tool-polygon = Polygon
vector-tool-line = Line
vector-tool-custom-shape = Custom Shape
vector-kind-rectangle = Rectangle
vector-kind-ellipse = Ellipse
vector-kind-triangle = Triangle
vector-kind-polygon = Polygon
vector-kind-line = Line
vector-kind-shape = Shape
vector-layer-name = { $kind } { $number }

## Vector tools: options bar

vector-mode-shape = Shape
vector-mode-path = Path
vector-mode-tip = Pick tool mode: a shape layer, or a path
vector-fill = Fill
vector-fill-tip = Set shape fill type
vector-stroke = Stroke
vector-stroke-tip = Set shape stroke type
vector-paint-none = No Color
vector-paint-color = Solid Color
vector-paint-gradient = Gradient
vector-stroke-width = Set shape stroke width
vector-stroke-options = Stroke Options
vector-stroke-align = Align
vector-align-inside = Inside
vector-align-center = Center
vector-align-outside = Outside
vector-stroke-caps = Caps
vector-cap-butt = Butt
vector-cap-round = Round
vector-cap-square = Square
vector-stroke-corners = Corners
vector-join-miter = Miter
vector-join-round = Round
vector-join-bevel = Bevel
vector-dashed-line = Dashed Line
vector-dash = Dash
vector-gap = Gap
vector-op-new-layer = New Layer
vector-op-combine = Combine Shapes
vector-op-subtract = Subtract Front Shape
vector-op-intersect = Intersect Shape Areas
vector-op-exclude = Exclude Overlapping Shapes
vector-corner-radius = Corner radius
vector-sides = Sides
vector-star-ratio = Star Ratio
vector-star-ratio-tip = Inner radius of the star, in percent of the outer one (100%: a polygon)
vector-weight = Weight
vector-weight-tip = Width of the line
vector-arrow-start = Start arrowhead
vector-arrow-end = End arrowhead
vector-custom-label = Shape
vector-custom-arrow = Arrow
vector-custom-heart = Heart
vector-custom-star = Star
vector-custom-check = Check Mark
vector-custom-bubble = Speech Bubble
vector-custom-bolt = Lightning
vector-auto-add-delete = Auto Add/Delete
vector-auto-add-delete-tip = A click on a segment of the selected path adds an anchor point, on an anchor point deletes it
vector-pen-hint = Click: corner point · drag: smooth point · the first point closes the path
vector-freeform-hint = Drag to draw a path
vector-anchor-tools-hint = Click the selected path
vector-path-select-hint = Click a path to select it, drag to move it
vector-direct-select-hint = Drag an anchor point, a handle or a segment
vector-shape-hint = Drag to draw a shape

## Vector tools: status bar hints

vector-hint-corner = corner point
vector-hint-smooth = smooth point
vector-hint-one-handle = one handle
vector-hint-end-path = end the path
vector-hint-selection = load as a selection
vector-hint-add-anchor = add a point
vector-hint-delete-anchor = delete a point
vector-hint-select-component = select a path
vector-hint-duplicate = duplicate
vector-hint-delete = delete
vector-hint-select-anchor = select a point
vector-hint-square = square / circle
vector-hint-draw-shape = draw

## Vector tools: history steps

vector-hist-new-anchor = New Anchor Point
vector-hist-add-anchor = Add Anchor Point
vector-hist-delete-anchor = Delete Anchor Point
vector-hist-convert-point = Convert Point
vector-hist-close-path = Close Path
vector-hist-drag-anchor = Drag Anchor Point
vector-hist-drag-handle = Drag Direction Point
vector-hist-drag-segment = Drag Segment
vector-hist-move-path = Drag Path
vector-hist-duplicate-path = Duplicate Path
vector-hist-delete-path = Delete Path
vector-hist-nudge = Nudge
vector-hist-path-operation = Path Operation
vector-hist-edit-shape = Shape Properties
vector-hist-rasterize = Rasterize Layer
vector-hist-new-path = New Path
vector-hist-save-path = Save Path
vector-hist-rename-path = Rename Path
vector-hist-stroke-path = Stroke Path
vector-hist-make-work-path = Make Work Path

## Shape layers: Properties panel, Layer menu

vector-live-shape = Live Shape
vector-appearance = Appearance
vector-radius-top-left = Top left corner radius
vector-radius-top-right = Top right corner radius
vector-radius-bottom-left = Bottom left corner radius
vector-radius-bottom-right = Bottom right corner radius
vector-radii-linked = Same radius for every corner
vector-menu-rasterize = Rasterize Layer
vector-status-rasterized = "{ $name }" rasterized (no longer editable as text or shape)

## Paths panel and its dialogs

vector-menu-paths = Paths
vector-paths-title = Paths
vector-paths-empty = No path yet: draw one with the Pen.
vector-work-path = Work Path
vector-path-name = Path { $number }
vector-shape-path-name = { $name } Shape Path
vector-path-name-label = Name
vector-new-path-tip = Create new path
vector-delete-path-tip = Delete current path
vector-make-selection-title = Make Selection
vector-fill-path-title = Fill Path
vector-stroke-path-title = Stroke Path
vector-make-work-path-title = Make Work Path
vector-rendering = Rendering
vector-operation = Operation
vector-op-new-selection = New Selection
vector-op-add-selection = Add to Selection
vector-op-subtract-selection = Subtract from Selection
vector-op-intersect-selection = Intersect with Selection
vector-stroke-tool = Tool
vector-simulate-pressure = Simulate Pressure
vector-simulate-pressure-tip = The stroke tapers at both ends, as with a pen tablet
vector-tolerance = Tolerance
vector-tolerance-tip = How far the path may stray from the selection's edge: the larger, the fewer anchor points
vector-status-no-path = No path: draw one, or select one in the Paths panel.
vector-status-no-selection = Make a selection first.

## Type tool: editing on the canvas

type-status-typing = Type your text
type-status-committed = Text committed
type-status-cancelled = Text edit cancelled
type-click-hint = Click to type a point text, drag to draw a paragraph text box

## Type tool: status bar hints

type-hint-point = point text
type-hint-paragraph = paragraph text
type-hint-new = new type layer over another
type-hint-move = move the text
type-hint-commit = commit
type-hint-extend = extend the selection

## Character panel

type-font-style-tip = Set the font style
type-size-tip = Set the font size
type-leading = Leading
type-leading-tip = Set the leading
type-leading-auto = Auto leading
type-leading-auto-tip = Leading at 120% of the font size
type-kerning = Kerning
type-kerning-metrics = Metrics
type-kerning-tip = Set the kerning between two characters
type-tracking = Tracking
type-tracking-tip = Set the tracking for the selected characters (thousandths of an em)
type-baseline = Shift
type-baseline-tip = Set the baseline shift
type-vscale = Vertical
type-vscale-tip = Vertically scale
type-hscale = Horizontal
type-hscale-tip = Horizontally scale
type-faux-bold = Faux Bold
type-faux-italic = Faux Italic
type-all-caps = All Caps
type-small-caps = Small Caps
type-underline = Underline
type-strikethrough = Strikethrough
type-aa = Anti-aliasing
type-aa-tip = Set the anti-aliasing method
type-aa-none = None
type-aa-sharp = Sharp
type-aa-smooth = Smooth

## Paragraph panel

type-paragraph = Paragraph
type-align-justify = Justify last left
type-indent-left = Left
type-indent-left-tip = Indent left margin
type-indent-right = Right
type-indent-right-tip = Indent right margin
type-indent-first = First line
type-indent-first-tip = Indent first line
type-space-before = Before
type-space-before-tip = Add space before paragraph
type-space-after = After
type-space-after-tip = Add space after paragraph

## History step names (History panel)

xchg-hist-type-layer = Type Layer
xchg-hist-edit-type = Edit Type Layer
xchg-hist-split = Split into Elements
xchg-hist-make-transparent = Make a Color Transparent
xchg-hist-align = Align
xchg-hist-distribute = Distribute
xchg-hist-move = Move
xchg-hist-fill = Fill
xchg-hist-canvas-size = Canvas Size
xchg-hist-image-size = Image Size
xchg-hist-rotate-canvas = Rotate Canvas
xchg-hist-new-adjustment-layer = New { $name } Layer
xchg-hist-modify-layer = Modify { $name } Layer
xchg-hist-layer-order = Layer Order
xchg-hist-link-layers = Link Layers
xchg-hist-unlink-layers = Unlink Layers
xchg-hist-add-mask = Add Layer Mask
xchg-hist-delete-mask = Delete Layer Mask
xchg-hist-apply-mask = Apply Layer Mask
xchg-hist-link-mask = Link Layer Mask
xchg-hist-unlink-mask = Unlink Layer Mask
xchg-hist-invert-mask = Invert
xchg-hist-mask-properties = Mask Properties
xchg-hist-lock-layer = Lock Layer
xchg-hist-unlock-layer = Unlock Layer
xchg-hist-show-layer = Show Layer
xchg-hist-hide-layer = Hide Layer
xchg-hist-rename-layer = Rename Layer
xchg-hist-opacity = Opacity Change
xchg-hist-fill-opacity = Fill Opacity Change
xchg-hist-blending = Blending Change
xchg-hist-clear-style = Clear Layer Style
xchg-hist-new-guide = New Guide
xchg-hist-move-guide = Move Guide
xchg-hist-delete-guide = Delete Guide
xchg-hist-clear-guides = Clear Guides
xchg-hist-save-selection = Save Selection
xchg-hist-select-and-mask = Select and Mask
xchg-hist-new-channel = New Channel
xchg-hist-duplicate-channel = Duplicate Channel
xchg-hist-delete-channel = Delete Channel
xchg-hist-channel-properties = Channel Options

## History panel

xchg-menu-history = History
xchg-history-title = History

## Channels panel

xchg-menu-channels = Channels
xchg-channels-title = Channels
xchg-channel-rgb = RGB
xchg-channel-red = Red
xchg-channel-green = Green
xchg-channel-blue = Blue
xchg-channel-alpha-name = Alpha { $number }
xchg-channel-name = Name
xchg-channel-duplicate = Duplicate Channel
xchg-channel-delete = Delete Channel
xchg-channel-load-tip = Load channel as selection
xchg-channel-save-tip = Save selection as channel
xchg-channel-new-tip = Create new channel
xchg-channel-delete-tip = Delete current channel

## Gradients of layers (fill, Gradient Map, Gradient Overlay)

xchg-gradient-edit-tip = Click to edit the gradient

## PSD files

xchg-file-filter-psd = PSD document
xchg-file-error-not-psd = Not a PSD document, or one this version can't read.
xchg-export-psd-hint = Layers, masks, groups, adjustment and fill layers and layer styles are kept, with the flattened image.
xchg-status-opened-simplified = Opened: { $path } — { $notes }
xchg-status-saved-simplified = Saved: { $path } — { $notes }
xchg-status-exported-simplified = Exported: { $path } — { $notes }
xchg-psd-alpha-name = Alpha { $number }
xchg-psd-mode-bitmap = Bitmap
xchg-psd-mode-grayscale = Grayscale
xchg-psd-mode-indexed = Indexed Color
xchg-psd-mode-cmyk = CMYK
xchg-psd-mode-multichannel = Multichannel
xchg-psd-mode-duotone = Duotone
xchg-psd-mode-lab = Lab
xchg-psd-note-depth = { $bits }-bit channels converted to 8 bits
xchg-psd-note-mode = { $mode } converted to RGB
xchg-psd-note-text = type layers as pixels: { $names }
xchg-psd-note-smart = smart objects as pixels: { $names }
xchg-psd-note-vector = vector masks rasterized: { $names }
xchg-psd-note-shapes = shape strokes left out: { $names }
xchg-psd-note-styles = layer styles simplified: { $names }
xchg-psd-note-dropped = not supported, left out: { $names }
xchg-psd-note-channels = { $count ->
        [one] one alpha channel left out (56 channels at most)
       *[other] { $count } alpha channels left out (56 channels at most)
    }
xchg-psd-note-text-saved = { $count ->
        [one] one type layer saved as pixels
       *[other] { $count } type layers saved as pixels
    }

## Color management: menus

color-menu-color-settings = Color Settings…
color-menu-assign-profile = Assign Profile…
color-menu-convert-profile = Convert to Profile…
color-menu-proof-setup = Proof Setup
color-menu-proof-custom = Custom…
color-menu-proof-working-cmyk = Working CMYK
color-menu-proof-colors = Proof Colors
color-menu-gamut-warning = Gamut Warning
color-menu-mode = Mode
color-menu-mode-grayscale = Grayscale
color-menu-mode-rgb = RGB Color
color-menu-mode-cmyk = CMYK Color
color-mode-cmyk-tip = Documents are edited in RGB. For a CMYK file, use Export As with Use Proof Setup: the file is converted to the proof profile (Working CMYK by default).

## Color management: shared labels

color-profile = Profile
color-intent = Intent
color-intent-perceptual = Perceptual
color-intent-saturation = Saturation
color-intent-relative = Relative Colorimetric
color-intent-absolute = Absolute Colorimetric
color-bpc = Use Black Point Compensation
color-conversion-options = Conversion Options
color-load-title = Load a Color Profile
color-filter-profiles = Color profiles

## Color management: Color Settings

color-settings-title = Color Settings
color-settings-working-spaces = Working Spaces
color-settings-rgb = RGB
color-settings-cmyk = CMYK
color-settings-load-cmyk = Load CMYK…
color-settings-hint = Documents without a profile are shown and converted as the working RGB. The working CMYK is the printing condition of soft proofs, CMYK files and the Color Picker's CMYK values. Monitor profile: { $monitor }.

## Color management: Assign Profile, Convert to Profile

color-assign-title = Assign Profile
color-assign-label = Assign Profile
color-assign-none = Don't Color Manage This Document
color-assign-working = Working RGB: { $name }
color-assign-hint = The numbers stay the same: the colors look different. To keep their look, use Convert to Profile.
color-convert-title = Convert to Profile
color-convert-source = Source Space
color-convert-destination = Destination Space

## Color management: Proof Setup

color-proof-title = Customize Proof Condition
color-proof-device = Device to Simulate
color-proof-working = Working CMYK - { $name }
color-proof-load = Load…
color-proof-display = Display Options (On-Screen)
color-proof-paper = Simulate Paper Color
color-proof-ink = Simulate Black Ink

## Color management: Grayscale, history, messages

color-grayscale-discard = Discard color information?
color-grayscale-discard-ok = Discard
color-hist-assign-profile = Assign Profile
color-hist-convert-to-profile = Convert to Profile
color-hist-grayscale = Grayscale
color-status-assigned = Profile assigned: { $name }.
color-status-unmanaged = The document is no longer color managed: shown as the working RGB.
color-status-converted = Converted to { $name }.
color-status-not-a-profile = { $name } is not a usable color profile of that kind.
color-picker-gamut = Warning: out of gamut for printing. Click to select the closest printable color.
color-psd-note-cmyk-layers = layers converted to CMYK: blend modes and adjustments may look different

## Color management: Export As in CMYK

color-export-use-proof = Use Proof Setup: { $name }

## Layer › Smart Objects, File › Place

smart-menu = Smart Objects
smart-menu-convert = Convert to Smart Object
smart-menu-new-via-copy = New Smart Object via Copy
smart-menu-reset-transform = Reset Transform
smart-menu-update-modified = Update Modified Content
smart-menu-edit-contents = Edit Contents
smart-menu-export-contents = Export Contents…
smart-menu-embed-linked = Embed Linked
smart-menu-replace-contents = Replace Contents…
smart-menu-rasterize = Rasterize
smart-menu-place-embedded = Place Embedded…
smart-menu-place-linked = Place Linked…
smart-kind = Smart Object

## Smart filters: Filter menu, Layers panel, Blending Options

smart-menu-convert-for-filters = Convert for Smart Filters
smart-filters = Smart Filters
smart-menu-edit-filter = Edit Smart Filter
smart-menu-filter-blending = Edit Smart Filter Blending Options…
smart-menu-disable-filter = Disable Smart Filter
smart-menu-enable-filter = Enable Smart Filter
smart-menu-delete-filter = Delete Smart Filter
smart-menu-disable-filters = Disable Smart Filters
smart-menu-enable-filters = Enable Smart Filters
smart-menu-clear-filters = Clear Smart Filters
smart-menu-add-filter-mask = Add Filter Mask
smart-menu-delete-filter-mask = Delete Filter Mask
smart-menu-disable-filter-mask = Disable Filter Mask
smart-menu-enable-filter-mask = Enable Filter Mask
smart-blending-title = Blending Options ({ $name })
smart-blending-tip = Blending Options
smart-filter-row-tip = Double-click to edit the filter, drag to move it

## History steps

smart-hist-place-embedded = Place Embedded
smart-hist-place-linked = Place Linked
smart-hist-replace = Replace Contents
smart-hist-update = Update Smart Object
smart-hist-update-links = Update Modified Content
smart-hist-show-filter = Enable Smart Filter
smart-hist-hide-filter = Disable Smart Filter
smart-hist-move-filter = Move Smart Filter
smart-hist-blending = Smart Filter Blending Options
smart-hist-add-filter-mask = Add Filter Mask

## Status bar

smart-status-converted = "{ $name }" converted to a smart object
smart-status-locked = A locked layer cannot be converted.
smart-status-no-adjustments = Adjustments don't apply to a smart object: use an adjustment layer clipped to it.
smart-status-not-editable = The smart object isn't directly editable: rasterize it first.
smart-status-placed = Placed: { $path }
smart-status-replaced = Contents replaced: { $path }
smart-status-updated = Smart object updated: { $name }
smart-status-parent-gone = The smart object's document is closed: save these contents as a file.
smart-status-link-missing = Linked file missing, its last copy shown: { $names }
smart-status-links-updated = Linked files updated: { $names }

## Filter › Liquify: the workspace and its tools

liquify-title = Liquify
liquify-tool-forward-warp = Forward Warp
liquify-tool-reconstruct = Reconstruct
liquify-tool-smooth = Smooth
liquify-tool-twirl = Twirl Clockwise
liquify-tool-pucker = Pucker
liquify-tool-bloat = Bloat
liquify-tool-push-left = Push Left
liquify-tool-freeze = Freeze Mask
liquify-tool-thaw = Thaw Mask

## Liquify: Brush Tool Options

liquify-brush-options = Brush Tool Options
liquify-size = Size
liquify-density = Density
liquify-pressure = Pressure
liquify-rate = Rate
liquify-stylus-pressure = Stylus Pressure
liquify-pin-edges = Pin Edges

## Liquify: Brush Reconstruct Options, Revert Reconstruction

liquify-reconstruct-options = Brush Reconstruct Options
liquify-reconstruct = Reconstruct…
liquify-restore-all = Restore All
liquify-revert-title = Revert Reconstruction
liquify-amount = Amount

## Liquify: Mask Options

liquify-mask-options = Mask Options
liquify-mask-none = None
liquify-mask-all = Mask All
liquify-invert-all = Invert All

## Liquify: Load Mesh Options

liquify-load-mesh-options = Load Mesh Options
liquify-load-mesh = Load Mesh…
liquify-load-last-mesh = Load Last Mesh
liquify-save-mesh = Save Mesh…
liquify-load-mesh-title = Load Mesh
liquify-save-mesh-title = Save Mesh
liquify-mesh-files = Liquify Meshes
# Proposed name of a saved mesh.
liquify-mesh-file-name = Mesh
liquify-status-mesh-saved = Mesh saved: { $file }
liquify-status-mesh-error = The mesh could not be read or written: { $error }
liquify-status-not-a-mesh = This file is not a Liquify mesh.

## Liquify: View Options

liquify-view-options = View Options
liquify-show-mesh = Show Mesh
liquify-mesh-size = Size
liquify-mesh-small = Small
liquify-mesh-medium = Medium
liquify-mesh-large = Large
liquify-color = Color
liquify-show-mask = Show Mask
liquify-show-backdrop = Show Backdrop
liquify-backdrop-mode = Mode
liquify-backdrop-in-front = In Front
liquify-backdrop-behind = Behind
liquify-backdrop-blend = Blend
liquify-backdrop-opacity = Opacity

## Liquify: mesh and mask colors

liquify-color-red = Red
liquify-color-orange = Orange
liquify-color-yellow = Yellow
liquify-color-green = Green
liquify-color-cyan = Cyan
liquify-color-blue = Blue
liquify-color-magenta = Magenta
liquify-color-gray = Gray

## Actions panel

actions-title = Actions
actions-empty = No actions. Create one with the New Action button.
actions-toggle-tip = Toggle item on/off
actions-dialog-tip = Toggle dialog on/off
actions-stop-tip = Stop playing/recording
actions-record-tip = Begin recording
actions-play-tip = Play selection
actions-new-set-tip = Create new set
actions-new-action-tip = Create new action
actions-delete-tip = Delete
actions-menu-tip = Actions panel menu

## Actions panel menu

actions-menu-button-mode = Button Mode
actions-menu-new-action = New Action…
actions-menu-new-set = New Set…
actions-menu-duplicate = Duplicate
actions-menu-delete = Delete
actions-menu-play = Play
actions-menu-record = Start Recording
actions-menu-record-again = Record Again…
actions-menu-insert-stop = Insert Stop…
actions-menu-options = Action Options…
actions-menu-clear-all = Clear All Actions
actions-menu-reset = Reset Actions
actions-menu-load = Load Actions…
actions-menu-replace = Replace Actions…
actions-menu-save = Save Actions…
actions-file-filter = Atelier actions

## Actions panel dialogs

actions-new-action-title = New Action
actions-options-title = Action Options
actions-new-set-title = New Set
actions-set-options-title = Set Options
actions-insert-stop-title = Record Stop
actions-record = Record
actions-name = Name
actions-set = Set
actions-action = Action
actions-function-key = Function Key
actions-key-none = None
actions-key-taken = This key is used by a command.
actions-stop-message = Message
actions-allow-continue = Allow Continue
actions-continue = Continue
actions-stop = Stop
actions-append = Append
actions-reset-question = Replace current actions with the default actions?
actions-delete-set-question = Delete the set "{ $name }"?
actions-delete-action-question = Delete the action "{ $name }"?
actions-delete-step-question = Delete the step "{ $name }"?
actions-set-default-name = Set { $n }
actions-action-default-name = Action { $n }
actions-copy-name = { $name } copy

## Default actions

actions-default-set = Default Actions
actions-default-vignette = Vignette (selection)
actions-default-sepia = Sepia Toning (layer)
actions-default-resize = Resize to 1080 px
actions-default-web = Export for Web (JPEG)

## Steps of the actions

actions-step-rotate = Rotate Canvas
actions-step-crop = Crop
actions-step-select = Set Selection
actions-step-select-all = Select All
actions-step-inverse = Inverse
actions-step-export = Export
actions-step-stop = Stop
actions-step-unknown = { $name } (unknown command)
actions-yes = Yes
actions-no = No
actions-scale = Scale
actions-folder = Folder
actions-document-folder = The document’s folder

## Playing actions

actions-status-played = Action "{ $name }" played.
actions-status-stopped = Action "{ $name }" stopped.
actions-status-loaded = Actions loaded from { $path }.
actions-status-load-failed = Could not load the actions: { $error }
actions-status-saved = Actions saved to { $path }.
actions-status-save-failed = Could not save the actions: { $error }
actions-error-step = "{ $step }" (action "{ $action }") could not be played: { $error }
actions-error-no-document = no document is open.
actions-error-no-selection = there is no selection.
actions-error-unknown = this version of Atelier doesn’t know this command.
actions-error-no-folder = the document was never saved: it has no folder.
actions-error-exists = { $name } already exists.
actions-error-format = { $name } can’t be saved in its format.
actions-error-not-actions = Not an actions file.

## File › Automate, File › Scripts

actions-menu-automate = Automate
actions-menu-scripts = Scripts
actions-batch-menu = Batch…
actions-fit-image-menu = Fit Image…
actions-processor-menu = Image Processor…
actions-fit-image-title = Fit Image
actions-constrain-within = Constrain Within
actions-dont-enlarge = Don’t Enlarge

## Batch

actions-batch-title = Batch
actions-batch-play = Play
actions-batch-source = Source
actions-batch-destination = Destination
actions-batch-errors = Errors
actions-source-folder = Folder
actions-source-opened = Opened Files
actions-subfolders = Include All Subfolders
actions-choose = Choose…
actions-no-folder = No folder chosen
actions-no-file = No file chosen
actions-save-as = Save As…
actions-destination-none = None
actions-destination-save-close = Save and Close
actions-destination-folder = Folder
actions-save-close-hint = Each document replaces its file, in its format.
actions-file-naming = File Naming
actions-example = Example
actions-example-name = MyFile
actions-starting-serial = Starting serial #
actions-overwrite = Replace existing files
actions-errors-stop = Stop for Errors
actions-errors-log = Log Errors to File
actions-name-none = (none)
actions-name-text = Custom text
actions-name-document = Document Name
actions-name-document-lower = document name
actions-name-document-upper = DOCUMENT NAME
actions-name-serial = { $digits }-Digit Serial Number
actions-name-letter-lower = Serial Letter (a, b, c…)
actions-name-letter-upper = Serial Letter (A, B, C…)
actions-name-date = yyyymmdd (date)
actions-name-extension-lower = extension
actions-name-extension-upper = EXTENSION
actions-batch-progress = Document { $n } of { $total }
actions-batch-nothing = There is no image to process.
actions-batch-stopped = The batch stopped: { $error }
actions-batch-cancelled = Batch cancelled after { $count ->
    [one] 1 document
   *[other] { $count } documents
}.
actions-batch-done = Batch done: { $count ->
    [one] 1 document
   *[other] { $count } documents
}.
actions-batch-done-errors = Batch done: { $count ->
    [one] 1 document
   *[other] { $count } documents
}, { $errors ->
    [one] 1 error
   *[other] { $errors } errors
} (see { $path }).

## Image Processor

actions-processor-title = Image Processor
actions-processor-source = Select the images to process
actions-processor-location = Select location to save processed images
actions-same-location = Save in Same Location
actions-other-folder = Select Folder
actions-processor-subfolders-hint = Each format goes into a folder of its own (JPEG, PSD, TIFF).
actions-processor-unsaved = Some open documents were never saved: choose a folder.
actions-processor-file-type = File Type
actions-save-as-format = Save as { $format }
actions-resize-to-fit = Resize to Fit
actions-processor-preferences = Preferences
actions-run-action = Run Action
actions-copyright = Copyright Info
actions-convert-profile-srgb = Convert Profile to sRGB
actions-include-icc = Include ICC Profile
actions-run = Run

## File › Export

actions-menu-export = Export
actions-quick-export = Quick Export as PNG
actions-export-file-settings = File Settings
actions-export-image-size = Image Size
actions-export-gif-hint = 256 colors at most; each pixel is either transparent or opaque.
actions-export-metadata = Metadata
actions-metadata-none = None
actions-metadata-copyright = Copyright and Contact Info
actions-export-color-space = Color Space
actions-convert-srgb = Convert to sRGB
actions-embed-profile = Embed Color Profile

## Camera Raw: window, menu, files

raw-filter-title = Camera Raw Filter
raw-window-title = Camera Raw
raw-open = Open
raw-developing = Developing
raw-file-filter = Camera Raw
raw-note-preview-only = its RAW data can't be read: the camera's embedded preview (8 bits) is used
raw-preview-tip = Drag to move around; double-click to zoom to 100% or fit.
raw-zoom-fit = Fit
raw-zoom-fit-tip = Fit in View
raw-before = Before
raw-after = After
raw-before-tip = Toggle between Before and After
raw-side-by-side-tip = Before / After side by side
raw-clip-shadows-tip = Shadow clipping warning
raw-clip-highlights-tip = Highlight clipping warning
raw-profile = Profile
raw-profile-color = Color
raw-profile-monochrome = Monochrome
raw-panel-reset-tip = Reset this panel

## Camera Raw: panels

raw-panel-basic = Basic
raw-panel-curve = Tone Curve
raw-panel-mixer = Color Mixer
raw-panel-grading = Color Grading
raw-panel-detail = Detail
raw-panel-effects = Effects
raw-panel-lens = Lens
raw-panel-presets = Presets

## Camera Raw: Basic

raw-white-balance = White Balance
raw-wb-as-shot = As Shot
raw-wb-auto = Auto
raw-wb-daylight = Daylight
raw-wb-cloudy = Cloudy
raw-wb-shade = Shade
raw-wb-tungsten = Tungsten
raw-wb-fluorescent = Fluorescent
raw-wb-flash = Flash
raw-wb-custom = Custom
raw-wb-tool = White Balance Tool
raw-wb-tool-tip = Click a neutral area (gray or white) to balance the image on it.
raw-temperature = Temperature
raw-tint = Tint
raw-exposure = Exposure
raw-contrast = Contrast
raw-highlights = Highlights
raw-shadows = Shadows
raw-whites = Whites
raw-blacks = Blacks
raw-texture = Texture
raw-clarity = Clarity
raw-dehaze = Dehaze
raw-vibrance = Vibrance
raw-saturation = Saturation

## Camera Raw: Tone Curve

raw-curve-parametric = Parametric
raw-curve-point = Point
raw-curve-lights = Lights
raw-curve-darks = Darks

## Camera Raw: Color Mixer

raw-mixer-hue = Hue
raw-mixer-saturation = Saturation
raw-mixer-luminance = Luminance
raw-mixer-gray-hint = Monochrome: each color range lightens or darkens in the gray.
raw-hue-red = Red
raw-hue-orange = Orange
raw-hue-yellow = Yellow
raw-hue-green = Green
raw-hue-aqua = Aqua
raw-hue-blue = Blue
raw-hue-purple = Purple
raw-hue-magenta = Magenta

## Camera Raw: Color Grading

raw-grading-shadows = Shadows
raw-grading-midtones = Midtones
raw-grading-highlights = Highlights
raw-grading-global = Global
raw-grading-hue = Hue
raw-grading-saturation = Saturation
raw-grading-luminance = Luminance
raw-grading-blending = Blending
raw-grading-balance = Balance
raw-wheel-tip = Drag to choose the hue and saturation; double-click to reset.

## Camera Raw: Detail

raw-sharpening = Sharpening
raw-sharpen-amount = Amount
raw-sharpen-radius = Radius
raw-sharpen-detail = Detail
raw-sharpen-masking = Masking
raw-noise-reduction = Noise Reduction
raw-nr-luminance = Luminance
raw-nr-luminance-detail = Luminance Detail
raw-nr-color = Color
raw-nr-color-detail = Color Detail

## Camera Raw: Effects and Lens

raw-vignetting = Post Crop Vignetting
raw-vignette-style = Style
raw-vignette-highlight-priority = Highlight Priority
raw-vignette-color-priority = Color Priority
raw-vignette-paint-overlay = Paint Overlay
raw-amount = Amount
raw-midpoint = Midpoint
raw-roundness = Roundness
raw-feather = Feather
raw-grain = Grain
raw-grain-amount = Amount
raw-grain-size = Size
raw-grain-roughness = Roughness
raw-lens-distortion = Distortion
raw-lens-vignetting = Vignetting

## Camera Raw: Presets

raw-preset-vivid = Vivid
raw-preset-matte = Soft Matte
raw-preset-black-white = Black & White
raw-preset-warm-film = Warm Film
raw-preset-save = Save Preset
raw-preset-name = Name
raw-preset-delete = Delete Preset
raw-preset-save-failed = Could not save the presets: { $error }

## Artboards: the tool, the layers, the history

artboard-tool = Artboard
artboard-kind = Artboard
artboard-name = Artboard { $number }
artboard-hist-new = New Artboard
artboard-hist-duplicate = Duplicate Artboard
artboard-hist-move = Move Artboard
artboard-hist-resize = Resize Artboard
artboard-hist-background = Artboard Background
artboard-hist-from-group = Artboard from Group
artboard-hist-from-layers = Artboard from Layers
artboard-hist-ungroup = Ungroup Artboards

## Artboards: menus

artboard-menu-new-document = New…
artboard-menu-new = New Artboard…
artboard-menu-from-group = Artboard from Group…
artboard-menu-from-layers = Artboard from Layers…
artboard-menu-ungroup = Ungroup Artboards
artboard-menu-to-files = Artboards to Files…

## Artboards: sizes

artboard-preset-label = Preset
artboard-preset-custom = Custom
artboard-preset-phone = Phone
artboard-preset-phone-large = Large Phone
artboard-preset-tablet = Tablet
artboard-preset-tablet-large = Large Tablet
artboard-preset-web-small = Web Small
artboard-preset-web-medium = Web Medium
artboard-preset-web-large = Web Large
artboard-preset-social-square = Social Post, Square
artboard-preset-social-portrait = Social Post, Portrait
artboard-preset-social-story = Social Story
artboard-preset-social-landscape = Social Post, Landscape
artboard-portrait = Portrait
artboard-landscape = Landscape

## Artboards: options bar, Properties panel, status bar

artboard-options-size = Size
artboard-options-add = Add New Artboard
artboard-options-add-tip = Add an artboard of the same size to the right
artboard-options-hint = Drag on the pasteboard to draw an artboard
artboard-background = Background
artboard-background-other = Other
artboard-background-tip = Artboard background color
artboard-hint-draw = draw an artboard
artboard-hint-duplicate = "+": duplicate with its content

## Artboards: dialogs

artboard-dialog-new = New Artboard
artboard-dialog-from-group = Artboard from Group
artboard-dialog-from-layers = Artboard from Layers
artboard-name-label = Name
artboard-new-document-title = New Document
artboard-new-document-create = Create
artboard-new-document-artboards = Artboards
artboard-new-document-contents = Background Contents
artboard-files-title = Artboards to Files
artboard-files-destination = Destination
artboard-files-no-folder = No folder chosen
artboard-files-browse = Browse…
artboard-files-prefix = File Name Prefix
artboard-files-background = Include Background in Export
artboard-files-content-only = Artboard Content Only
artboard-files-overlapping = Include Overlapping Areas
artboard-files-hint = One file per artboard, named after it.
artboard-export-as-list = Artboards (one file each)

## Artboards: status messages

artboard-status-new-document = New document: { $width } × { $height } px
artboard-status-not-group = Select a group outside any artboard first.
artboard-status-none = The document has no artboards.
artboard-status-refused = Cannot make that artboard (locked layers, or an artboard among them).
artboard-status-exporting = Exporting { $count ->
    [one] { $count } artboard…
   *[other] { $count } artboards…
}
artboard-status-exported = { $count ->
    [one] { $count } artboard exported to { $folder }
   *[other] { $count } artboards exported to { $folder }
}
artboard-status-export-failed = Export failed: { $error }

## Image Processor

actions-use-proof = Use Proof Setup
actions-use-proof-tip = JPEG and TIFF files converted to the document’s proof profile (CMYK)

## Color management: profile names

color-adobe-rgb-compatible = Compatible with Adobe RGB (1998)

## History panel: selection steps (named like the commands)
hist3-deselect = Deselect
hist3-select-all = Select All
hist3-reselect = Reselect
hist3-inverse = Select Inverse
hist3-move-selection = Move Selection
hist3-select-subject = Select Subject
hist3-make-selection = Make Selection
hist3-selection = Selection Change
hist3-quick-mask = Quick Mask

## History panel: first row once older steps were forgotten
hist3-oldest-state = Oldest State Kept

## Selection computed in the background
hist3-selection-changed = The selection changed while the previous one was being computed: that result was dropped.

## Move tool options
input3-auto-select = Auto-Select
input3-auto-select-tip = A click selects the layer under the pointer. Hold { $key } to do the opposite for a moment.

## Hints and tips with modifier keys (written by the app for each system)
input3-eyedropper-hint = Click: foreground · { $alt_click }: background
input3-visibility-tip = Show / hide ({ $alt_click }: this one only)
input3-curves-hint = Click to add a point; drag it off the graph (or { $ctrl_click }, { $delete }) to remove it. Arrows: move the point, { $next }: next point.
input3-layer-row-tip = Drag to reorder, or into a group · double-click on the name: rename, beside it: layer style · { $alt_click }: clip to the layer below · { $ctrl_click } on a thumbnail: its pixels or mask as the selection · on the mask: { $shift_click } turns it off or on, { $alt_click } shows it alone
input3-clone-hint = { $alt_click } to set the source, then paint to copy it
input3-healing-hint = { $alt_click } to set the source, then paint: its texture blends into the colors around
input3-channel-row-tip = Click: show alone · { $ctrl_click }: load as selection · Double-click: rename
input3-filter-mask-tip = Filter mask: click to edit it, { $shift_click } to turn it off or on
input3-add-mask-tip = Add layer mask ({ $alt }: hide all)
input3-fill-path-tip = Fill path with foreground color ({ $alt }: options)
input3-stroke-path-tip = Stroke path with brush ({ $alt }: options)
input3-make-selection-tip = Load path as a selection ({ $alt }: options)
input3-make-work-path-tip = Make work path from selection ({ $alt }: tolerance)
input3-raw-sharpen-hint = Hold { $alt } while dragging a sharpening slider to see what it does. Sharpening and noise reduction show from 100%.

## Status bar hints of the Move tool
input3-hint-axis = horizontally or vertically
input3-hint-copy = move a copy
input3-hint-pick-layer = the layer under the pointer
input3-hint-selected-layers = the selected layers

## Saving over a Photoshop document

files3-quoted-name = "{ $name }"
files3-psd-note-shapes = shape layers saved as pixels: { $names }
files3-status-psd-would-lose = A Photoshop file would lose: { $notes }. Save as an Atelier document to keep everything.
files3-dialog-save-as-lossy = Save As (an Atelier document keeps everything)

## Clipboard

files3-menu-copy-merged = Copy Merged
files3-menu-paste-special = Paste Special
files3-menu-paste-in-place = Paste in Place
files3-menu-paste-into = Paste Into
files3-status-copied-merged = Visible layers copied
files3-status-paste-into-no-selection = Paste Into needs a selection.

## Export

files3-export-no-metadata = { $format } files hold no metadata.
files3-status-artboards-busy = Artboards are still being exported.

## Painting and retouching (fix3 tools)

tools3-status-source-empty = Nothing copied: the source area is empty (Sample: { $sample }).
tools3-status-layer-hidden = Could not paint: "{ $name }" is hidden.
tools3-rasterize-text = The type layer "{ $name }" must be rasterized before painting on it. Its text will no longer be editable.
tools3-rasterize-shape = The shape layer "{ $name }" must be rasterized before painting on it. Its shape will no longer be editable.
tools3-rasterize-ok = Rasterize
tools3-bucket-all-layers = All Layers
tools3-bucket-all-layers-tip = Fills by the colors all the visible layers show, on the active layer

## Layer › Remove Background (fix3 tools)

tools3-status-remove-background-pixels = Remove Background works on a pixel, type, shape or smart object layer.

## Actions panel: Clear All Actions, the actions file
smart3-actions-clear-all-question = Delete all actions?
smart3-actions-damaged = The saved actions could not be read ({ $error }): the default actions are shown, and the file was kept as { $backup }.
smart3-actions-unreadable = The saved actions could not be read ({ $error }): the default actions are shown, and changes to actions won't be saved until Atelier restarts.
smart3-actions-not-recorded = Not recorded: this command can't be part of an action.
smart3-actions-not-saved = The actions could not be saved: { $error }

## Batch
smart3-batch-written-twice = { $name } was already written by this batch: change the File Naming (add a serial number).

## Filters, Liquify, Camera Raw
fx3-filter-failed = { $name } could not be applied: the layer is unchanged.
fx3-filter-layer-hidden = "{ $name }" is hidden: show it to filter it.

## Image › Mode › Grayscale
fx3-grayscale-note = Colors become grays of the same lightness, and color-only adjustments are neutralized. The document stays in RGB mode: colors painted later stay in color.

## Proof Colors on: next to the zoom in the document tab
fx3-tab-proof = { $zoom } (RGB/8/CMYK)

## AI models: downloads
fx3-download-cancel-tip = Cancel the download of { $name }
fx3-ai-error-offline = no connection to the server: check the Internet connection
fx3-ai-error-status = the server answered with error { $code }
fx3-ai-error-secure = the secure connection to the server failed
fx3-ai-error-disk-full = the disk is full
fx3-ai-error-no-write = cannot write to the models folder ({ $folder })
fx3-ai-error-write = the file could not be written ({ $error })
fx3-model-ready-run-again = { $name } is ready: run the command again (the selection or the layer changed meanwhile).

## Content-Aware Fill, Spot Healing Brush
fx3-ai-skipped-transparency = AI model not used: the area has transparent pixels, filled with patches instead.

## Start screen
ui3-status-welcome = Create an image ({ $new }), open one ({ $open }) or drop it into the window.

## File menu: recent files
ui3-menu-open-recent = Open Recent
ui3-menu-clear-recent = Clear Recent File List

## Window menu: workspace
ui3-menu-hide-panels = Hide Panels
ui3-menu-reset-workspace = Reset Workspace

## Help menu
ui3-menu-help = Help
ui3-menu-shortcuts = Keyboard Shortcuts…
ui3-menu-website = Atelier Website
ui3-menu-report-problem = Report a Problem…
ui3-menu-about = About Atelier

## About window
ui3-about-version = Version { $version }
ui3-about-description = A light image editor with familiar shortcuts.
ui3-about-license = License: { $license }
ui3-about-source = Source code

## Keyboard Shortcuts window
ui3-shortcuts-title = Keyboard Shortcuts
ui3-shortcuts-filter = Search
ui3-shortcuts-other = Other keys
ui3-shortcuts-palette = Search commands
ui3-shortcuts-hide-side = Hide the side panel
ui3-shortcuts-brush-size = Brush size (smaller / larger)
ui3-shortcuts-pan = Pan the view (hold)

## Error alert
ui3-alert-title = Something went wrong

## Command palette
ui3-palette-more = { $count ->
    [one] { $count } more result: keep typing to narrow down
   *[other] { $count } more results: keep typing to narrow down
}
# Why a greyed command can't run
ui3-why-no-image = Open or create an image first
ui3-why-no-selection = Make a selection first
ui3-why-nothing-to-undo = Nothing to undo
ui3-why-nothing-to-redo = Nothing to redo
ui3-why-not-now = Not available for the active layer or in the current state
# Other words a command is found by (space-separated, never shown)
ui3-palette-keywords-image-size = resize scale resample dimensions enlarge shrink
ui3-palette-keywords-canvas-size = extend expand border margin
ui3-palette-keywords-rotate = rotate turn flip mirror
ui3-palette-keywords-crop = crop trim cut
ui3-palette-keywords-move = move
ui3-palette-keywords-type = type font write
ui3-palette-keywords-black-white = black white grayscale greyscale monochrome
ui3-palette-keywords-export = jpg jpeg png webp gif save web
ui3-palette-keywords-save = psd
ui3-palette-keywords-cut-out = cut out cutout isolate erase background transparent
ui3-palette-keywords-shortcuts = keys keyboard hotkeys help
ui3-palette-keywords-about = version help license credits
ui3-palette-keywords-recent = recent file open

## Export As: file sizes

words3-size-kb = { $size } KB

## A label and its value (action steps, batch errors)

words3-label-value = { $label }: { $value }
