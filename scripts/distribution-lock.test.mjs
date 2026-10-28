import test from 'node:test';
import assert from 'node:assert/strict';
import {createHash} from 'node:crypto';
import {refreshLocalRunnerLock} from './distribution-lock.mjs';

test('owned runner refresh updates integrity while conserving registry pins', () => {
  const registry={version:'3.4.5',resolved:'https://registry.example/pkg.tgz',integrity:'sha512-fixed'};
  const original={version:'0.1.0',lockfileVersion:3,packages:{'':{version:'0.1.0',dependencies:{'@apx/runner':'file:vendor/apx-runner-0.1.0.tgz',pkg:'3.4.5'}},'node_modules/@apx/runner':{version:'0.1.0',integrity:'old'},'node_modules/pkg':registry}};
  const manifest={version:'0.2.0',dependencies:{'@apx/runner':'file:vendor/apx-runner-0.2.0.tgz',pkg:'3.4.5'},bundleDependencies:['@apx/runner','pkg']};
  const bytes=Buffer.from('new owned runner archive');
  const result=refreshLocalRunnerLock(original,manifest,{version:'0.2.0'},bytes);
  assert.deepEqual(result.packages['node_modules/pkg'],registry);
  assert.equal(result.packages['node_modules/@apx/runner'].integrity,`sha512-${createHash('sha512').update(bytes).digest('base64')}`);
  assert.equal(original.packages['node_modules/@apx/runner'].integrity,'old');
  assert.throws(()=>refreshLocalRunnerLock(original,{...manifest,dependencies:{...manifest.dependencies,pkg:'3.4.6'}},{version:'0.2.0'},bytes),/dependencies changed/);
});
