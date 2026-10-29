import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { spawnSync } from 'node:child_process';
import { createRequire } from 'node:module';
import { refreshLocalRunnerLock } from './distribution-lock.mjs';
import {nativeTargets,executableName,verifyNative,observeNative,loadEngineBundle,sha256} from './native-targets.mjs';
const require = createRequire(import.meta.url);
const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const args = process.argv.slice(2);
const option = (name, fallback) => { const i = args.indexOf(name); return i < 0 ? fallback : args[i + 1]; };
const target = option('--target', `${process.platform}-${process.arch}`);
if (!['universal',...nativeTargets].includes(target)) throw new Error(`Unsupported packaging target ${target}`);
if (target==='universal' && (!option('--engine-dir') || args.includes('--engine'))) throw new Error('Universal packaging requires --engine-dir and does not accept --engine.');
if (target!=='universal' && args.includes('--engine-dir')) throw new Error('--engine-dir is only supported with --target universal.');
const engine = path.resolve(option('--engine', path.join(root, 'target', 'release', process.platform === 'win32' ? 'apx.exe' : 'apx')));
const output = path.resolve(option('--output', path.join(root, 'dist')));
const npm = process.env.APX_NPM_CLI || process.env.npm_execpath;
if (!npm || !fs.existsSync(npm)) throw new Error('Run via npm run package or set APX_NPM_CLI to npm-cli.js.');
if (target!=='universal' && !fs.existsSync(engine)) throw new Error(`Build the native engine first: ${engine}`);
const manifests=['package.json','packages/runner/package.json','packages/sf-plugin/package.json','packages/vscode/package.json'].map(file=>JSON.parse(fs.readFileSync(path.join(root,file))));
const version=manifests[0].version;
const nativeVersion=fs.readFileSync(path.join(root,'crates/apx-engine/Cargo.toml'),'utf8').match(/^version\s*=\s*"([^"]+)"/m)?.[1];
if (manifests.some(manifest=>manifest.version!==version)||nativeVersion!==version) throw new Error('Workspace, native engine, runner, plugin and extension versions must match.');
const engines = target==='universal' ? loadEngineBundle(path.resolve(option('--engine-dir')),version) : [{target,engine}];
for (const entry of engines) {
  const bytes=fs.readFileSync(entry.engine);
  entry.info=verifyNative(bytes,entry.target,{requireStaticLinux:target==='universal'});
  entry.sha256=sha256(bytes);
  if(target!=='universal' && entry.target===`${process.platform}-${process.arch}`) entry.receipt=observeNative(entry.engine,entry.target,version,{requireStaticLinux:false});
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
  for(const entry of engines) {
    const destination = path.join(folder, 'bin', entry.target, executableName(entry.target));
    copy(entry.engine, destination);
    if(sha256(fs.readFileSync(destination))!==entry.sha256) throw new Error(`Native ${entry.target} bytes changed during packaging.`);
    if (!entry.target.startsWith('win32')) fs.chmodSync(destination, 0o755);
    if(entry.receipt) fs.writeFileSync(path.join(path.dirname(destination),'engine.json'),JSON.stringify(entry.receipt,null,2)+'\n');
  }
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
  // The checked-in lock pins external dependencies. Repacked owned-runner
  // integrity belongs to this artifact, not a host-specific source mutation.
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
run(process.execPath, [vsce, 'package', '--no-dependencies', '--allow-missing-repository', '--skip-license', ...(target==='universal'?[]:['--target',target]), '--out', vsix], extension);
fs.writeFileSync(path.join(stagingRoot, 'latest.json'), JSON.stringify({ stagedExtension: extension, stagedPlugin: plugin, vsix }, null, 2));
fs.writeFileSync(path.join(output, 'packaging-manifest.json'), JSON.stringify({ schemaVersion:2, version: extensionManifest.version, target, targets:engines.map(entry=>entry.target), ...(target==='universal'?{}:{engine:path.basename(engine)}), engines:engines.map(entry=>({target:entry.target,file:`bin/${entry.target}/${executableName(entry.target)}`,sha256:entry.sha256,format:entry.info.format,linkage:entry.info.linkage,receipt:entry.receipt??null})), vsix: path.basename(vsix), plugin:`apx-sf-plugin-${version}.tgz`, assurance: 'experimental-local-execution', qualification: 'not Salesforce-qualified' }, null, 2)+'\n');
console.log(`Packaged ${vsix}`);
