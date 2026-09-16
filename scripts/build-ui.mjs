import { createHash } from "node:crypto";
import { copyFile, lstat, mkdir, readFile, readdir, realpath, stat, writeFile } from "node:fs/promises";
import { constants as fsConstants } from "node:fs";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";

const MAX_FILE_BYTES = 2 * 1024 * 1024;
const MAX_TOTAL_BYTES = 16 * 1024 * 1024;
const ALLOWED_EXTENSIONS = new Set([
  ".css",
  ".html",
  ".ico",
  ".js",
  ".json",
  ".mjs",
  ".svg",
  ".webmanifest",
]);

const scriptDirectory = path.dirname(fileURLToPath(import.meta.url));
const projectRoot = path.resolve(scriptDirectory, "..");
const defaultSource = path.join(projectRoot, "src", "interfaces", "ui", "public");

function fail(message) {
  process.stderr.write(`build-ui: ${message}\n`);
  process.exitCode = 1;
}

function parseArguments(argv) {
  const values = new Map();
  for (let index = 0; index < argv.length; index += 2) {
    const flag = argv[index];
    const value = argv[index + 1];
    if (!flag?.startsWith("--") || value === undefined || value.startsWith("--")) {
      throw new Error("expected --output-root PATH with an optional --source PATH");
    }
    if (values.has(flag)) {
      throw new Error(`duplicate option: ${flag}`);
    }
    values.set(flag, value);
  }
  for (const flag of values.keys()) {
    if (flag !== "--output-root" && flag !== "--source") {
      throw new Error(`unknown option: ${flag}`);
    }
  }
  const outputRoot = values.get("--output-root");
  if (!outputRoot) {
    throw new Error("--output-root is required");
  }
  return {
    outputRoot: path.resolve(outputRoot),
    source: path.resolve(values.get("--source") ?? defaultSource),
  };
}

function isInside(parent, candidate) {
  const relative = path.relative(parent, candidate);
  return relative === "" || (!relative.startsWith("..") && !path.isAbsolute(relative));
}

async function assertExternalOutput(outputRoot) {
  if (isInside(projectRoot, outputRoot)) {
    throw new Error(`output must be outside the source directory: ${outputRoot}`);
  }

  let existing = outputRoot;
  while (true) {
    try {
      await stat(existing);
      break;
    } catch (error) {
      if (error?.code !== "ENOENT") {
        throw error;
      }
      const parent = path.dirname(existing);
      if (parent === existing) {
        throw new Error("output path has no existing ancestor");
      }
      existing = parent;
    }
  }

  const resolvedExisting = await realpath(existing);
  if (isInside(projectRoot, resolvedExisting)) {
    throw new Error(`output resolves inside the source directory: ${resolvedExisting}`);
  }

  let ancestor = existing;
  while (true) {
    const info = await lstat(ancestor);
    if (info.isSymbolicLink()) {
      throw new Error(`linked output path is not supported: ${ancestor}`);
    }
    const parent = path.dirname(ancestor);
    if (parent === ancestor) {
      break;
    }
    ancestor = parent;
  }

  await mkdir(outputRoot, { recursive: true });
  const outputInfo = await lstat(outputRoot);
  if (!outputInfo.isDirectory() || outputInfo.isSymbolicLink()) {
    throw new Error(`output root must be a regular directory: ${outputRoot}`);
  }
  const resolvedOutput = await realpath(outputRoot);
  if (isInside(projectRoot, resolvedOutput)) {
    throw new Error(`output resolves inside the source directory: ${resolvedOutput}`);
  }
  return resolvedOutput;
}

async function ensureSafeDirectory(outputRoot, resolvedOutput, segments) {
  let current = outputRoot;
  for (const segment of segments) {
    current = path.join(current, segment);
    let info;
    try {
      info = await lstat(current);
    } catch (error) {
      if (error?.code !== "ENOENT") {
        throw error;
      }
      await mkdir(current);
      info = await lstat(current);
    }
    if (!info.isDirectory() || info.isSymbolicLink()) {
      throw new Error(`linked or non-directory output path is not supported: ${current}`);
    }
    const resolved = await realpath(current);
    if (!isInside(resolvedOutput, resolved)) {
      throw new Error(`output directory resolves outside the build root: ${current}`);
    }
  }
  return current;
}

async function copyContentAddressed(source, destination, expectedDigest) {
  try {
    await copyFile(
      source,
      destination,
      fsConstants.COPYFILE_EXCL | fsConstants.COPYFILE_FICLONE,
    );
    return;
  } catch (error) {
    if (error?.code !== "EEXIST") {
      throw error;
    }
  }

  const info = await lstat(destination);
  if (!info.isFile() || info.isSymbolicLink()) {
    throw new Error(`existing output asset is not a regular file: ${destination}`);
  }
  const digest = createHash("sha256").update(await readFile(destination)).digest("hex");
  if (digest !== expectedDigest) {
    throw new Error(`existing content-addressed asset does not match: ${destination}`);
  }
}

async function writeContentAddressed(destination, contents) {
  try {
    await writeFile(destination, contents, { encoding: "utf8", flag: "wx" });
    return;
  } catch (error) {
    if (error?.code !== "EEXIST") {
      throw error;
    }
  }

  const info = await lstat(destination);
  if (!info.isFile() || info.isSymbolicLink()) {
    throw new Error(`existing manifest is not a regular file: ${destination}`);
  }
  if ((await readFile(destination, "utf8")) !== contents) {
    throw new Error(`existing content-addressed manifest does not match: ${destination}`);
  }
}

async function collectFiles(root, current = root) {
  const entries = await readdir(current, { withFileTypes: true });
  const files = [];
  for (const entry of entries.sort((left, right) =>
    left.name < right.name ? -1 : left.name > right.name ? 1 : 0)) {
    const absolute = path.join(current, entry.name);
    if (entry.isSymbolicLink()) {
      throw new Error(`linked UI source is not supported: ${absolute}`);
    }
    if (entry.isDirectory()) {
      files.push(...(await collectFiles(root, absolute)));
      continue;
    }
    if (!entry.isFile()) {
      throw new Error(`unsupported UI source entry: ${absolute}`);
    }
    const extension = path.extname(entry.name).toLowerCase();
    if (!ALLOWED_EXTENSIONS.has(extension)) {
      throw new Error(`unsupported UI asset type: ${absolute}`);
    }
    files.push({
      absolute,
      relative: path.relative(root, absolute).split(path.sep).join("/"),
    });
  }
  return files;
}

async function build(source, outputRoot) {
  const sourceInfo = await lstat(source);
  if (!sourceInfo.isDirectory() || sourceInfo.isSymbolicLink()) {
    throw new Error(`UI source is not a directory: ${source}`);
  }
  const resolvedOutput = await assertExternalOutput(outputRoot);

  const files = await collectFiles(source);
  if (!files.some((file) => file.relative === "index.html")) {
    throw new Error("UI source must contain index.html");
  }

  const aggregate = createHash("sha256");
  const manifestFiles = [];
  let totalBytes = 0;
  for (const file of files) {
    const bytes = await readFile(file.absolute);
    if (bytes.byteLength > MAX_FILE_BYTES) {
      throw new Error(`UI asset exceeds ${MAX_FILE_BYTES} bytes: ${file.relative}`);
    }
    totalBytes += bytes.byteLength;
    if (totalBytes > MAX_TOTAL_BYTES) {
      throw new Error(`UI assets exceed ${MAX_TOTAL_BYTES} bytes in total`);
    }
    const digest = createHash("sha256").update(bytes).digest("hex");
    aggregate.update(file.relative);
    aggregate.update("\0");
    aggregate.update(digest);
    aggregate.update("\0");
    manifestFiles.push({ path: file.relative, bytes: bytes.byteLength, sha256: digest });
  }

  const buildId = aggregate.digest("hex").slice(0, 16);
  const destination = await ensureSafeDirectory(outputRoot, resolvedOutput, ["ui-dist", buildId]);

  for (const [index, file] of files.entries()) {
    const output = path.join(destination, ...file.relative.split("/"));
    const directorySegments = file.relative.split("/").slice(0, -1);
    await ensureSafeDirectory(destination, await realpath(destination), directorySegments);
    await copyContentAddressed(file.absolute, output, manifestFiles[index].sha256);
  }

  const stagedFiles = (await collectFiles(destination)).map((file) => file.relative);
  const expectedFiles = files.map((file) => file.relative);
  if (JSON.stringify(stagedFiles) !== JSON.stringify(expectedFiles)) {
    throw new Error("content-addressed output contains unexpected or missing assets");
  }

  const manifestDirectory = await ensureSafeDirectory(outputRoot, resolvedOutput, ["ui-manifests"]);
  const manifestPath = path.join(manifestDirectory, `${buildId}.json`);
  const manifest = {
    schema_version: 1,
    build_id: buildId,
    source: "src/interfaces/ui/public",
    total_bytes: totalBytes,
    files: manifestFiles,
  };
  await writeContentAddressed(manifestPath, `${JSON.stringify(manifest, null, 2)}\n`);

  process.stdout.write(
    `${JSON.stringify({ status: "ok", build_id: buildId, output_dir: destination, manifest: manifestPath, files: files.length })}\n`,
  );
}

try {
  const { source, outputRoot } = parseArguments(process.argv.slice(2));
  await build(source, outputRoot);
} catch (error) {
  fail(error instanceof Error ? error.message : String(error));
}
