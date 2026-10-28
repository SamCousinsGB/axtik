import assert from 'node:assert/strict';
import { test } from 'node:test';
import { spawnSync } from 'node:child_process';
import { existsSync } from 'node:fs';
import { mkdir, mkdtemp, writeFile, rm } from 'node:fs/promises';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const packageRoot = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const tempRoot = resolve(packageRoot, '../..', '.tmp', 'plugin');
const cli = join(packageRoot, 'bin', 'run.js');
const env = { ...process.env, SF_DISABLE_TELEMETRY: 'true', SFDX_DISABLE_TELEMETRY: 'true', SF_AUTOUPDATE_DISABLE: 'true', SF_DISABLE_LOG_FILE: 'true', NO_COLOR: '1' };
const invoke = (args, cwd = packageRoot) => spawnSync(process.execPath, [cli, ...args], { cwd, encoding: 'utf8', env, windowsHide: true, timeout: 30_000 });

test('oclif discovers all three actual SfCommand commands', () => {
  for (const name of ['check', 'validate', 'test']) {
    const result = invoke(['apx', name, '--help']);
    assert.equal(result.status, 0, result.stderr); assert.match(result.stdout, /--source-dir/); assert.match(result.stdout, /--json/);
  }
});
test('command flags reject invalid deadlines', () => {
  const result = invoke(['apx', 'check', '--timeout-ms', '0', '--json']);
  assert.notEqual(result.status, 0); assert.match(result.stdout, /timeout-ms/);
});
test('JSON wrapper and process retain native preview exit four', async t => {
  await mkdir(tempRoot, { recursive: true }); const dir = await mkdtemp(join(tempRoot, 'case-'));
  t.after(async () => { assert.ok(resolve(dir).startsWith(tempRoot + (process.platform === 'win32' ? '\\' : '/'))); await rm(dir, { recursive: true, force: true }); });
  const report = {
    schemaVersion: 1, engineVersion: 'fixture', command: 'validate', status: 'preview_passed',
    assurance: 'experimental-local-execution', exitCode: 4, projectRoot: dir,
    files: [], diagnostics: [], tests: [], limitations: ['Test fixture'],
    summary: { files: 0, selectedFiles: 0, tests: 0, passed: 0, failed: 0, blocked: 0, errored: 0, durationMs: 0 },
  };
  await writeFile(join(dir, 'validate'), `const r=${JSON.stringify(report)};r.limitations=[JSON.stringify(process.argv.slice(2))];process.stdout.write(JSON.stringify(r));process.exitCode=4;`);
  const result = invoke(['apx', 'validate', '--project', dir, '--engine', process.execPath, '--source-dir', 'file with spaces.cls', '--environment', 'fixtures/local & permissions.json', '--json']);
  assert.equal(result.status, 4, result.stderr); const output = JSON.parse(result.stdout);
  assert.equal(output.status, 4); assert.equal(output.result.exitCode, 4); assert.equal(output.result.status, 'preview_passed');
  const args = JSON.parse(output.result.limitations[0]);
  assert.equal(args[args.indexOf('--environment') + 1], resolve(dir, 'fixtures/local & permissions.json'));
});
test('missing native engine yields a structured diagnostic', () => {
  const result = invoke(['apx', 'check', '--engine', resolve(tempRoot, 'missing.exe'), '--json']);
  assert.notEqual(result.status, 0); assert.doesNotThrow(() => JSON.parse(result.stdout));
});

test('omitting --project preserves native project discovery', async t => {
  await mkdir(tempRoot, { recursive: true }); const dir = await mkdtemp(join(tempRoot, 'discovery-'));
  t.after(async () => { assert.ok(resolve(dir).startsWith(tempRoot + (process.platform === 'win32' ? '\\' : '/'))); await rm(dir, { recursive: true, force: true }); });
  const report = {
    schemaVersion: 1, engineVersion: 'fixture', command: 'check', status: 'checked',
    assurance: 'syntax-and-declarations', exitCode: 0, projectRoot: dir,
    files: [], diagnostics: [], tests: [], limitations: [],
    summary: { files: 0, selectedFiles: 0, tests: 0, passed: 0, failed: 0, blocked: 0, errored: 0, durationMs: 0 },
  };
  await writeFile(join(dir, 'check'), `const r=${JSON.stringify(report)};r.limitations=[JSON.stringify(process.argv.slice(2))];process.stdout.write(JSON.stringify(r));`);
  const result = invoke(['apx', 'check', '--engine', process.execPath, '--json'], dir);
  assert.equal(result.status, 0, result.stderr);
  const args = JSON.parse(JSON.parse(result.stdout).result.limitations[0]);
  assert.ok(!args.includes('--project')); assert.ok(args.includes(dir));
});

const nativeName = process.platform === 'win32' ? 'apx.exe' : 'apx';
const nativeEngine = [process.env.APX_ENGINE_PATH, join(packageRoot, 'bin', `${process.platform}-${process.arch}`, nativeName), resolve(packageRoot, '../../target/release', nativeName)].find(p => p && existsSync(p));
test('native engine discovers ancestor dependencies and honors an explicit project without source paths', { skip: !nativeEngine }, async t => {
  await mkdir(tempRoot, { recursive: true }); const dir = await mkdtemp(join(tempRoot, 'nested-'));
  t.after(async () => { assert.ok(resolve(dir).startsWith(tempRoot + (process.platform === 'win32' ? '\\' : '/'))); await rm(dir, { recursive: true, force: true }); });
  const nested = join(dir, 'force-app', 'main', 'default', 'classes', 'tests');
  const helpers = join(dir, 'force-app', 'main', 'default', 'classes', 'helpers');
  await mkdir(nested, { recursive: true }); await mkdir(helpers, { recursive: true });
  await writeFile(join(dir, 'sfdx-project.json'), JSON.stringify({ packageDirectories: [{ path: 'force-app', default: true }], namespace: '', sourceApiVersion: '65.0' }));
  await writeFile(join(helpers, 'SiblingHelper.cls'), 'public class SiblingHelper { public static Integer value() { return 3; } }');
  await writeFile(join(nested, 'NestedTest.cls'), '@IsTest private class NestedTest { @IsTest static void sibling() { System.assertEquals(3, SiblingHelper.value()); } }');
  const result = invoke(['apx', 'validate', '--engine', nativeEngine, '--json'], nested);
  assert.equal(result.status, 4, result.stderr);
  const report = JSON.parse(result.stdout).result;
  assert.equal(report.status, 'preview_passed'); assert.equal(resolve(report.projectRoot), dir);
  assert.equal(report.summary.files, 2); assert.equal(report.summary.passed, 1); assert.equal(report.summary.blocked, 0);
  const explicit = invoke(['apx', 'test', '--engine', nativeEngine, '--project', dir, '--filter', 'NestedTest.sibling', '--json']);
  assert.equal(explicit.status, 4, explicit.stderr);
  const projectReport = JSON.parse(explicit.stdout).result;
  assert.equal(projectReport.status, 'preview_passed'); assert.equal(resolve(projectReport.projectRoot), dir);
  assert.equal(projectReport.summary.files, 2); assert.equal(projectReport.summary.tests, 1); assert.equal(projectReport.summary.passed, 1);
});
