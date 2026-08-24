// Rasterise the FDM logo into every raster format Windows, Tauri, and Chrome need.
//
// SVG is the source of truth (brand/logo/*.svg); nothing here should ever be
// hand-edited. Run `npm install && npm run rasterize` in this directory after
// changing a logo.
//
// Two source marks, deliberately:
//   fdm-icon.svg        red mark on the black squircle  -> app icon, .ico, tray
//   fdm-mark-small.svg  arrow only, transparent          -> <=32px toolbar icons
// Below about 24px the three parallel bars smear into a solid block, which is
// exactly the thing the mark is supposed to communicate, so the small variant
// drops them. See brand/BRAND.md section 4.

import { mkdir, writeFile } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import pngToIco from "png-to-ico";
import sharp from "sharp";

const HERE = dirname(fileURLToPath(import.meta.url));
const ROOT = resolve(HERE, "..");
const LOGO = join(ROOT, "brand", "logo");

const OUT_ICONS = join(ROOT, "brand", "icons");
const OUT_TAURI = join(ROOT, "apps", "desktop", "src-tauri", "icons");
const OUT_EXT = join(ROOT, "extension", "icons");
const OUT_INSTALLER = join(ROOT, "installer", "assets");

const ICON_SRC = join(LOGO, "fdm-icon.svg");
const MARK_SRC = join(LOGO, "fdm-mark.svg");
const SMALL_SRC = join(LOGO, "fdm-mark-small.svg");

// sharp cannot write BMP, and Inno Setup cannot read anything else for its
// wizard art. 24-bit BMP is a 54-byte header over bottom-up BGR rows padded to
// a 4-byte boundary — cheaper to encode here than to add another dependency.
//
// `channels` is a parameter and not assumed to be 3: compositing anything with
// an alpha channel makes sharp promote its output to RGBA, and hardcoding a
// 3-byte stride against 4-byte data reads every row at a sliding offset. The
// result is diagonal magenta stripes, which is exactly what shipped the first
// time this was written.
function encodeBmp24(raw, width, height, channels) {
  const rowSize = (width * 3 + 3) & ~3; // BMP rows are 4-byte aligned
  const pixels = Buffer.alloc(rowSize * height);

  for (let y = 0; y < height; y++) {
    const srcRow = y * width * channels;
    const dstRow = (height - 1 - y) * rowSize; // BMP rows run bottom-up
    for (let x = 0; x < width; x++) {
      const s = srcRow + x * channels;
      const d = dstRow + x * 3;
      pixels[d + 0] = raw[s + 2]; // B
      pixels[d + 1] = raw[s + 1]; // G
      pixels[d + 2] = raw[s + 0]; // R
    }
  }

  const header = Buffer.alloc(54);
  header.write("BM", 0, "ascii");
  header.writeUInt32LE(54 + pixels.length, 2); // total file size
  header.writeUInt32LE(54, 10); // pixel data offset
  header.writeUInt32LE(40, 14); // BITMAPINFOHEADER size
  header.writeInt32LE(width, 18);
  header.writeInt32LE(height, 22);
  header.writeUInt16LE(1, 26); // planes
  header.writeUInt16LE(24, 28); // bits per pixel
  header.writeUInt32LE(pixels.length, 34); // raw image size
  return Buffer.concat([header, pixels]);
}

// Flatten the mark onto the brand black at a given canvas size, centred, and
// return a ready-to-write BMP. BMP has no alpha channel, hence the flatten.
async function wizardBmp(width, height, markSize, dest) {
  const BG = { r: 2, g: 6, b: 23 }; // --fdm-bg
  const mark = await sharp(ICON_SRC, { density: 384 })
    .resize(markSize, markSize)
    .png()
    .toBuffer();

  const { data, info } = await sharp({
    create: { width, height, channels: 3, background: BG },
  })
    .composite([
      {
        input: mark,
        top: Math.round((height - markSize) / 2),
        left: Math.round((width - markSize) / 2),
      },
    ])
    // Composite the alpha away against the brand black before going raw, so the
    // buffer handed to the encoder has no premultiplied edges to guess about.
    .flatten({ background: BG })
    .raw()
    .toBuffer({ resolveWithObject: true });

  if (info.width !== width || info.height !== height) {
    throw new Error(
      `wizard art geometry drifted: asked ${width}x${height}, sharp returned ${info.width}x${info.height}`,
    );
  }

  await mkdir(dirname(dest), { recursive: true });
  await writeFile(dest, encodeBmp24(data, info.width, info.height, info.channels));
  return dest;
}

// The squircle already carries the black, so the icon renders opaque. The
// transparent mark must stay transparent — Chrome composites it onto whatever
// colour the user's toolbar happens to be.
async function png(src, size, dest) {
  await mkdir(dirname(dest), { recursive: true });
  await sharp(src, { density: 384 }) // 384dpi keeps 512px sources crisp when upscaled
    .resize(size, size, { fit: "contain", background: { r: 0, g: 0, b: 0, alpha: 0 } })
    .png({ compressionLevel: 9, adaptiveFiltering: true })
    .toFile(dest);
  return dest;
}

// Under 32px use the arrow-only mark; at or above it, the full three-bar mark.
const srcFor = (size) => (size < 32 ? SMALL_SRC : ICON_SRC);

async function main() {
  const written = [];

  // 1. Reference set — every size anything downstream might ask for.
  const REFERENCE = [16, 20, 24, 32, 40, 48, 64, 96, 128, 256, 512, 1024];
  for (const size of REFERENCE) {
    written.push(await png(srcFor(size), size, join(OUT_ICONS, `fdm-${size}.png`)));
  }

  // 2. Windows .ico. 256 must be the last entry and PNG-compressed, which
  //    png-to-ico does; Explorer picks the nearest size rather than scaling,
  //    so a missing 24 or 48 shows up as a visibly blurry icon in some views.
  const icoSizes = [16, 24, 32, 48, 64, 128, 256];
  const icoBuffers = await Promise.all(
    icoSizes.map((size) =>
      sharp(srcFor(size), { density: 384 }).resize(size, size).png().toBuffer(),
    ),
  );
  const ico = await pngToIco(icoBuffers);
  for (const dest of [join(OUT_ICONS, "fdm.ico"), join(OUT_TAURI, "icon.ico")]) {
    await mkdir(dirname(dest), { recursive: true });
    await writeFile(dest, ico);
    written.push(dest);
  }

  // 3. Tauri's expected filenames. It will not build without these exact names.
  const TAURI = [
    [32, "32x32.png"],
    [128, "128x128.png"],
    [256, "128x128@2x.png"],
    [512, "icon.png"],
  ];
  for (const [size, name] of TAURI) {
    written.push(await png(ICON_SRC, size, join(OUT_TAURI, name)));
  }

  // 4. Chrome MV3 manifest icon set. These MUST all be transparent, never the
  //    squircle: Chrome swaps between the 16 and the 32 purely on display DPI,
  //    so if one were transparent and the other a black tile, the toolbar icon
  //    would change appearance when the window moved to another monitor.
  for (const size of [16, 32, 48, 128]) {
    const src = size < 32 ? SMALL_SRC : MARK_SRC;
    written.push(await png(src, size, join(OUT_EXT, `icon-${size}.png`)));
  }

  // 5. Inno Setup wizard art. BMP only — Inno cannot read PNG. WizardImageFile
  //    is 164x314 and WizardSmallImageFile is 55x58 at 100% scaling; Inno picks
  //    the @2x variants automatically on high-DPI displays if they are present,
  //    so both scales are generated.
  written.push(await wizardBmp(164, 314, 120, join(OUT_INSTALLER, "wizard-large.bmp")));
  written.push(await wizardBmp(328, 628, 240, join(OUT_INSTALLER, "wizard-large@2x.bmp")));
  written.push(await wizardBmp(55, 58, 44, join(OUT_INSTALLER, "wizard-small.bmp")));
  written.push(await wizardBmp(110, 116, 88, join(OUT_INSTALLER, "wizard-small@2x.bmp")));

  // The installer needs its own .ico next to the wizard art.
  await mkdir(OUT_INSTALLER, { recursive: true });
  await writeFile(join(OUT_INSTALLER, "fdm.ico"), ico);
  written.push(join(OUT_INSTALLER, "fdm.ico"));

  for (const f of written) console.log("  " + f.replace(ROOT + "\\", "").replace(ROOT + "/", ""));
  console.log(`\n${written.length} files written from 2 SVG sources.`);
}

main().catch((err) => {
  console.error("rasterize failed:", err.message);
  process.exit(1);
});
