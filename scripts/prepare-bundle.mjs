import { copyFile, mkdir } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const target = "x86_64-pc-windows-msvc";
const bin = resolve(root, "src-tauri", "bin");

await mkdir(bin, { recursive: true });
await copyFile(
  resolve(root, "node_modules", "ffmpeg-static", "ffmpeg.exe"),
  resolve(bin, `ffmpeg-${target}.exe`),
);
await copyFile(
  resolve(
    root,
    "node_modules",
    "ffprobe-static",
    "bin",
    "win32",
    "x64",
    "ffprobe.exe",
  ),
  resolve(bin, `ffprobe-${target}.exe`),
);

console.log("FFmpeg sidecars подготовлены для Tauri.");
