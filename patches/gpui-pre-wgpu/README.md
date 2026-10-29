# gpui-pre-wgpu Android compatibility patch

Source: crates.io `gpui-pre-wgpu` 0.3.9, the Zed `gpui_wgpu` snapshot at
`93d745668805029476e6d7fbb19652116fed51de`. Licensed under Apache-2.0;
see `LICENSE-APACHE`.

The Android emulator's GFXStream/MoltenVK shader translator generates incompatible
Metal array types when `prepare_gradient_color` receives an array member from a
storage-buffer record. Pipeline creation fails with
`LinearColorStop[2]` versus `spvUnsafeArray<LinearColorStop, 2>`, which wgpu reports
as device loss.

Pass the two gradient endpoint colors separately. This preserves the gradient
calculation and GPU buffer layout while avoiding the failing array argument.
`android-gradient.patch` contains the change against the published crate.

Remove this Cargo patch once the upstream snapshot includes the fix.
