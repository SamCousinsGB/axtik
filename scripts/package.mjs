import fs from 'node:fs';
import path from 'node:path';
import os from 'node:os';
import { fileURLToPath } from 'node:url';
import { spawnSync } from 'node:child_process';
import { createRequire } from 'node:module';
import { refreshLocalRunnerLock } from './distribution-lock.mjs';
const require = createRequire(import.meta.url);
const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const args = process.argv.slice(2);
const option = (name, fallback) => { const i = args.indexOf(name); return i < 0 ? fallback : args[i + 1]; };
const target = option('--target', `${process.platform}-${process.arch}`);
if (!['win32-x64', 'linux-x64', 'darwin-arm64'].includes(target)) throw new Error(`Unsupported packaging target ${target}`);
const engine = path.resolve(option('--engine', path.join(root, 'target', 'release', process.platform === 'win32' ? 'apx.exe' : 'apx')));
const output = path.resolve(option('--output', path.join(root, 'dist')));
const npm = process.env.APX_NPM_CLI || process.env.npm_execpath;
if (!npm || !fs.existsSync(npm)) throw new Error('Run via npm run package or set APX_NPM_CLI to npm-cli.js.');
if (!fs.existsSync(engine)) throw new Error(`Build the native engine first: ${engine}`);
const manifests=['package.json','packages/runner/package.json','packages/sf-plugin/package.json','packages/vscode/package.json'].map(file=>JSON.parse(fs.readFileSync(path.join(root,file))));
const version=manifests[0].version;
const nativeVersion=fs.readFileSync(path.join(root,'crates/apx-engine/Cargo.toml'),'utf8').match(/^version\s*=\s*"([^"]+)"/m)?.[1];
if (manifests.some(manifest=>manifest.version!==version)||nativeVersion!==version) throw new Error('Workspace, native engine, runner, plugin and extension versions must match.');
const header = fs.readFileSync(engine);
const nativeTarget = (() => {
  if (header.length >= 64 && header.toString('ascii', 0, 2) === 'MZ') {
    const pe = header.readUInt32LE(0x3c);
    if (pe + 6 <= header.length && header.readUInt32LE(pe) === 0x4550 && header.readUInt16LE(pe + 4) === 0x8664) return 'win32-x64';
  }
  if (header.length >= 20 && header.subarray(0, 4).equals(Buffer.from([0x7f, 0x45, 0x4c, 0x46])) && header[4] === 2 && header[5] === 1 && header.readUInt16LE(18) === 62) return 'linux-x64';
  if (header.length >= 8 && header.readUInt32LE(0) === 0xfeedfacf && header.readUInt32LE(4) === 0x0100000c) return 'darwin-arm64';
  return 'unknown';
})();
if (nativeTarget !== target) throw new Error(`Engine format is ${nativeTarget}; refusing to label it ${target}.`);
if (target===`${process.platform}-${process.arch}`) {
  const result=spawnSync(engine,['--version'],{shell:false,encoding:'utf8',timeout:10000,windowsHide:true});
  if(result.error || result.status!==0 || result.stdout.trim()!==`apx ${version}`) throw new Error('Native executable version does not match the release manifests. Rebuild it before packaging.');
}
fs.mkdirSync(output, { recursive: true });
const stagingRoot = path.join(root, '.packaging');
fs.mkdirSync(stagingRoot, { recursive: true });
const stage = fs.mkdtempSync(path.join(stagingRoot, 'release-'));
const plugin = path.join(stage, 'sf-plugin');
const extension = path.join(stage, 'extension');
fs.mkdirSync(plugin); fs.mkdirSync(extension);
function run(file, argv, cwd) {
  const result = spawnSync(file, argv, { cwd, shell: false, stdio: 'inherit', windowsHide: true });
  if (result.error) throw result.error;
  if (result.status !== 0) throw new Error(`${file} failed (${result.status})`);
}
function copy(from, to) { fs.mkdirSync(path.dirname(to), { recursive: true }); fs.cpSync(from, to, { recursive: true, dereference: true }); }
function native(folder) {
  const name = target.startsWith('win32') ? 'apx.exe' : 'apx';
  const destination = path.join(folder, 'bin', target, name);
  copy(engine, destination); if (!target.startsWith('win32')) fs.chmodSync(destination, 0o755);
}
// Pack the real runner rather than distributing a workspace symlink.
const vendor = path.join(plugin, 'vendor'); fs.mkdirSync(vendor);
run(process.execPath, [npm, 'pack', '--ignore-scripts', '--pack-destination', vendor], path.join(root, 'packages', 'runner'));
for (const name of ['lib', 'README.md']) copy(path.join(root, 'packages', 'sf-plugin', name), path.join(plugin, name));
copy(path.join(root, 'LICENSE'), path.join(plugin, 'LICENSE'));
const manifest = JSON.parse(fs.readFileSync(path.join(root, 'packages', 'sf-plugin', 'package.json')));
delete manifest.devDependencies; delete manifest.scripts;
manifest.dependencies['@apx/runner'] = `file:vendor/${fs.readdirSync(vendor).find(n => n.endsWith('.tgz'))}`;
manifest.files = ['lib', 'bin', 'vendor', 'README.md', 'LICENSE', 'NOTICE', 'THIRD_PARTY_NOTICES.txt'];
fs.writeFileSync(path.join(plugin, 'package.json'), JSON.stringify(manifest, null, 2));
native(plugin);
const distributionLock = path.join(root, 'packaging', 'sf-plugin.package-lock.json');
const reuseLock = fs.existsSync(distributionLock) && !args.includes('--refresh-package-lock');
if (reuseLock) {
  const runnerArchive=path.join(plugin,manifest.dependencies['@apx/runner'].slice(5));
  const updated=refreshLocalRunnerLock(JSON.parse(fs.readFileSync(distributionLock)),manifest,manifests[1],fs.readFileSync(runnerArchive));
  const text=JSON.stringify(updated,null,2)+'\n';
  fs.writeFileSync(path.join(plugin,'package-lock.json'),text);
  fs.writeFileSync(distributionLock,text);
}
run(process.execPath, [npm, reuseLock ? 'ci' : 'install', '--omit=dev', '--ignore-scripts', '--no-audit', '--no-fund'], plugin);
if (!reuseLock) copy(path.join(plugin, 'package-lock.json'), distributionLock);
// Every production dependency is physically present for offline SF CLI linking.
for (const name of ['NOTICE', 'THIRD_PARTY_NOTICES.txt']) {
  const source = path.join(root, name); if (fs.existsSync(source)) copy(source, path.join(plugin, name));
}
run(process.execPath, [npm, 'pack', '--ignore-scripts', '--pack-destination', output], plugin);
for (const name of ['dist', 'README.md']) copy(path.join(root, 'packages', 'vscode', name), path.join(extension, name));
const extensionManifest = JSON.parse(fs.readFileSync(path.join(root, 'packages', 'vscode', 'package.json')));
delete extensionManifest.scripts; delete extensionManifest.devDependencies; delete extensionManifest.dependencies;
extensionManifest.files = ['dist/**', 'bin/**', 'sf-plugin/**', 'README.md', 'LICENSE', 'NOTICE', 'THIRD_PARTY_NOTICES.txt'];
fs.writeFileSync(path.join(extension, 'package.json'), JSON.stringify(extensionManifest, null, 2));
copy(path.join(root, 'LICENSE'), path.join(extension, 'LICENSE'));
for (const name of ['NOTICE', 'THIRD_PARTY_NOTICES.txt']) {
  const source = path.join(root, name); if (fs.existsSync(source)) copy(source, path.join(extension, name));
}
native(extension); copy(plugin, path.join(extension, 'sf-plugin'));
const vscePackage = require.resolve('@vscode/vsce/package.json', { paths: [path.join(root, 'packages', 'vscode')] });
const vsce = path.join(path.dirname(vscePackage), 'vsce');
const vsix = path.join(output, `apx-local-apex-${extensionManifest.version}-${target}.vsix`);
run(process.execPath, [vsce, 'package', '--no-dependencies', '--allow-missing-repository', '--skip-license', '--target', target, '--out', vsix], extension);
fs.writeFileSync(path.join(stagingRoot, 'latest.json'), JSON.stringify({ stagedExtension: extension, stagedPlugin: plugin, vsix }, null, 2));
fs.writeFileSync(path.join(output, 'packaging-manifest.json'), JSON.stringify({ version: extensionManifest.version, target, engine: path.basename(engine), vsix: path.basename(vsix), assurance: 'experimental-local-execution', qualification: 'not Salesforce-qualified' }, null, 2));
console.log(`Packaged ${vsix}`);
