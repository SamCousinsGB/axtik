#!/usr/bin/env node
import fs from 'node:fs/promises';
import path from 'node:path';
import {fileURLToPath} from 'node:url';
import {execFileSync} from 'node:child_process';

const root = path.dirname(path.dirname(fileURLToPath(import.meta.url)));
const metadata = JSON.parse(execFileSync('cargo', ['metadata', '--format-version', '1', '--locked'], {
  cwd: root, encoding: 'utf8', maxBuffer: 16 * 1024 * 1024, windowsHide: true,
}));
const packages = metadata.packages.filter(p => p.source).sort((a, b) => `${a.name}@${a.version}`.localeCompare(`${b.name}@${b.version}`));
const blocks = ['APX third-party notices\nGenerated from the locked Cargo dependency graph and packaged license files.\nDependency licenses apply independently of APX original source.\nJavaScript production dependencies retain their license files in the bundled SF plugin.'];
const missing = [];
for (const p of packages) {
  const directory = path.dirname(p.manifest_path);
  const entries = await fs.readdir(directory, {withFileTypes:true});
  const names = entries.filter(e => e.isFile() && /^(licen[cs]e|copying|notice)([.\-_]|$)/i.test(e.name)).map(e => e.name);
  if (p.license_file) names.push(p.license_file);
  const contents = [];
  for (const name of [...new Set(names)].sort()) {
    const file = path.resolve(directory, name);
    if (path.relative(directory, file).startsWith('..')) throw new Error(`License escapes crate directory: ${p.name}`);
    const stat = await fs.stat(file);
    if (stat.size > 1024 * 1024) throw new Error(`License unexpectedly exceeds 1 MiB: ${p.name}/${name}`);
    contents.push(`${name}\n${(await fs.readFile(file,'utf8')).replaceAll('\r\n','\n').trim()}\n`);
  }
  if (!contents.length) {
    const bundled = path.join(root, 'third-party', `${p.name}-${p.version}`);
    try {
      const provenance = JSON.parse(await fs.readFile(path.join(bundled, 'source.json'),'utf8'));
      const vcs = JSON.parse(await fs.readFile(path.join(directory, '.cargo_vcs_info.json'),'utf8'));
      if (vcs.git.sha1 !== provenance.commit) throw new Error(`Pinned license revision mismatch: ${p.name}`);
      contents.push(`LICENSE (upstream ${provenance.url})\n${(await fs.readFile(path.join(bundled,'LICENSE'),'utf8')).trim()}\n`);
    } catch (error) { if (error.code !== 'ENOENT') throw error; }
  }
  if (!contents.length) missing.push(`${p.name}@${p.version}`);
  blocks.push(`${'='.repeat(72)}\n${p.name} ${p.version}\n${p.license ?? 'License metadata unavailable'}\n${p.repository ?? p.homepage ?? ''}\n\n${contents.join('\n')}`);
}
if (missing.length) throw new Error(`No packaged license text for: ${missing.join(', ')}. Review before release.`);
await fs.writeFile(path.join(root,'THIRD_PARTY_NOTICES.txt'), blocks.join('\n\n').trimEnd()+'\n');
console.log(`Updated notices for ${packages.length} locked Rust dependency packages.`);
