#!/usr/bin/env node
'use strict';

const { execSync } = require('child_process');
const {
  createWriteStream,
  existsSync,
  mkdirSync,
  chmodSync,
  writeFileSync,
  unlinkSync,
} = require('fs');
const { join } = require('path');
const https = require('https');

const pkg = require('./package.json');
const version = pkg.version;

const TARGETS = {
  'linux-x64': 'x86_64-unknown-linux-musl',
  'linux-arm64': 'aarch64-unknown-linux-musl',
  'darwin-x64': 'x86_64-apple-darwin',
  'darwin-arm64': 'aarch64-apple-darwin',
  'win32-x64': 'x86_64-pc-windows-msvc',
};

const platformKey = `${process.platform}-${process.arch}`;
const target = TARGETS[platformKey];
if (!target) {
  console.error(`archival: unsupported platform: ${platformKey}`);
  process.exit(1);
}

const isWindows = process.platform === 'win32';
const binExt = isWindows ? '.exe' : '';
const binDir = join(__dirname, 'bin');
const binPath = join(binDir, `archival${binExt}`);

const tagName = `v${version}`;
const tarName = `archival-${tagName}-${target}.tar.gz`;
const releaseDir = `archival-${tagName}-${target}`;
const url = `https://github.com/archival-dev/archival/releases/download/${tagName}/${tarName}`;
const tmpTar = join(binDir, tarName);

if (!existsSync(binDir)) mkdirSync(binDir, { recursive: true });

function download(url, dest) {
  return new Promise((resolve, reject) => {
    function get(currentUrl) {
      https
        .get(currentUrl, (res) => {
          if (res.statusCode === 301 || res.statusCode === 302) {
            return get(res.headers.location);
          }
          if (res.statusCode !== 200) {
            return reject(new Error(`HTTP ${res.statusCode}: ${currentUrl}`));
          }
          const file = createWriteStream(dest);
          res.pipe(file);
          file.on('finish', () => file.close(resolve));
          file.on('error', reject);
        })
        .on('error', reject);
    }
    get(url);
  });
}

console.log(`Downloading archival ${version} for ${target}...`);
download(url, tmpTar)
  .then(() => {
    try {
      const tarTarget = `${releaseDir}/archival${binExt}`;
      execSync(
        `tar xzf "${tmpTar}" -C "${binDir}" --strip-components=1 "${tarTarget}"`,
        { stdio: 'inherit' }
      );

      if (isWindows) {
        // The extracted binary is bin/archival.exe; create a .cmd shim so that
        // npm bin symlinks (which point to bin/archival) work in CMD/PowerShell.
        // Note: npm itself creates shims for the bin/archival entry that invoke
        // node, which won't work for a native binary. Use `bin\archival.exe`
        // directly on Windows if the npm bin shim does not work.
        writeFileSync(join(binDir, 'archival.cmd'), `@"%~dp0archival.exe" %*\r\n`);
      } else {
        chmodSync(binPath, 0o755);
      }

      unlinkSync(tmpTar);
      console.log('archival installed successfully.');
    } catch (err) {
      console.error(`archival: installation failed: ${err.message}`);
      process.exit(1);
    }
  })
  .catch((err) => {
    console.error(`archival: download failed: ${err.message}`);
    process.exit(1);
  });
