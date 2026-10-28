import { createHash } from 'node:crypto';

// Repackage only the owned workspace runner. Registry resolutions and integrity
// pins remain untouched; a dependency change requires an explicit lock refresh.
export function refreshLocalRunnerLock(lock, manifest, runnerManifest, archive) {
  if (lock.lockfileVersion !== 3 || !lock.packages?.[''] || !lock.packages['node_modules/@apx/runner']) throw new Error('Expected the pinned SF plugin distribution lock.');
  if (Object.keys(runnerManifest.dependencies ?? {}).length) throw new Error('Runner runtime dependencies require an explicit distribution lock refresh.');
  const external = dependencies => Object.fromEntries(Object.entries(dependencies ?? {}).filter(([name]) => name !== '@apx/runner').sort());
  if (JSON.stringify(external(lock.packages[''].dependencies)) !== JSON.stringify(external(manifest.dependencies))) throw new Error('SF plugin dependencies changed; refresh and review the distribution lock explicitly.');
  const reference = manifest.dependencies['@apx/runner'];
  if (reference !== `file:vendor/apx-runner-${runnerManifest.version}.tgz`) throw new Error('Runner archive and manifest versions disagree.');
  const updated = structuredClone(lock);
  updated.version = manifest.version;
  Object.assign(updated.packages[''], { version: manifest.version, dependencies: manifest.dependencies, bundleDependencies: manifest.bundleDependencies });
  Object.assign(updated.packages['node_modules/@apx/runner'], {
    version: runnerManifest.version,
    resolved: reference,
    integrity: `sha512-${createHash('sha512').update(archive).digest('base64')}`,
  });
  return updated;
}
