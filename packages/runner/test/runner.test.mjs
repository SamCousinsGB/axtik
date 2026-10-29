import assert from 'node:assert/strict';
import { test } from 'node:test';
import { createHash } from 'node:crypto';
import { mkdir, mkdtemp, writeFile, rm } from 'node:fs/promises';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { EngineRunError, runEngine, validateReport, resolveBundledEngine, inputFingerprint, directoryFingerprint, verifyReportInputs } from '../lib/index.js';

const tempRoot = resolve(dirname(fileURLToPath(import.meta.url)), '../../../..', '.tmp', 'runner');
const report = (command = 'check', code = command === 'check' ? 0 : 4) => ({
  schemaVersion: 2, engineVersion: 'fixture', command, status: command === 'check' ? 'checked' : 'preview_passed',
  assurance: command === 'check' ? 'syntax-and-declarations' : 'experimental-local-execution', exitCode: code,
  projectRoot: '.', inputs: [], inputDirectories: [], inputFingerprint: inputFingerprint([],[]), files: [], diagnostics: [], tests: [], limitations: ['fixture'],
  summary: { files: 0, selectedFiles: 0, tests: 0, passed: 0, failed: 0, blocked: 0, errored: 0, durationMs: 0 },
});

async function fixture(t, code, command = 'check') {
  await mkdir(tempRoot, { recursive: true });
  const dir = await mkdtemp(join(tempRoot, 'case-'));
  await writeFile(join(dir, command), code);
  t.after(async () => { assert.ok(resolve(dir).startsWith(tempRoot + (process.platform === 'win32' ? '\\' : '/'))); await rm(dir, { recursive: true, force: true }); });
  return { enginePath: process.execPath, command, paths: [], projectRoot: dir };
}

test('accepts a complete syntax-only report', () => assert.equal(validateReport(report()).status, 'checked'));
test('rejects a preview report claiming successful exit zero', () => assert.throws(() => validateReport(report('validate', 0)), /incorrectly labels/));
test('rejects incomplete reports', () => assert.throws(() => validateReport({ schemaVersion: 1 }), EngineRunError));
test('rejects a protocol version mismatch rather than silently losing snapshot and assertion guarantees', () => assert.throws(() => validateReport({ ...report(), schemaVersion: 1 }), EngineRunError));
test('rejects mismatched selection counts', () => assert.throws(() => validateReport({ ...report(), summary: { ...report().summary, files: 1 } }), /counts/));
test('rejects a checked label hiding an error diagnostic', () => {
  const r = report(); r.diagnostics.push({ code: 'BROKEN', severity: 'error', message: 'parse failed', path: '', line: 1, column: 1, endLine: 1, endColumn: 1 });
  assert.throws(() => validateReport(r), /contradicts/);
});
test('accepts discovery-only test identities in check reports', () => {
  const r = report(); r.tests = [{ id: 'Example.test', path: 'Example.cls', line: 1, status: 'not_run', message: '', durationMs: 0, assertions:{executed:0,passed:0,failed:0}, setupAssertions:{executed:0,passed:0,failed:0} }]; r.summary.tests = 1;
  assert.equal(validateReport(r).tests[0].status, 'not_run');
});
test('does not accept discovery-only tests as execution results', () => {
  const r = report('test'); r.tests = [{ id: 'Example.test', path: 'Example.cls', line: 1, status: 'not_run', message: '', durationMs: 0, assertions:{executed:0,passed:0,failed:0}, setupAssertions:{executed:0,passed:0,failed:0} }]; r.summary.tests = 1;
  assert.throws(() => validateReport(r), /never accounted/);
});
test('executes without a shell and preserves arguments with metacharacters', async t => {
  const options = await fixture(t, `const r=${JSON.stringify(report())};r.limitations=[JSON.stringify(process.argv.slice(2))];process.stdout.write(JSON.stringify(r));`);
  options.paths = ['file with spaces & ; quote\'s.cls']; options.filter = 'Example.*';
  const result = await runEngine(options); const args = JSON.parse(result.limitations[0]);
  assert.ok(args.includes(resolve(options.projectRoot, options.paths[0]))); assert.ok(args.includes('Example.*'));
});

test('an explicit environment is one project-relative argument and is never auto-added', async t => {
  const options = await fixture(t, `const r=${JSON.stringify(report())};r.limitations=[JSON.stringify(process.argv.slice(2))];process.stdout.write(JSON.stringify(r));`);
  const omitted = JSON.parse((await runEngine(options)).limitations[0]);
  assert.ok(!omitted.includes('--environment'));
  const selected = JSON.parse((await runEngine({...options, environmentPath: 'fixtures/local & test.json'})).limitations[0]);
  assert.equal(selected[selected.indexOf('--environment') + 1], resolve(options.projectRoot, 'fixtures/local & test.json'));
});
test('preserves experimental process exit four', async t => {
  const options = await fixture(t, `process.stdout.write(${JSON.stringify(JSON.stringify(report('validate')))});process.exitCode=4;`, 'validate');
  assert.equal((await runEngine(options)).exitCode, 4);
});
test('rejects exit/report mismatches', async t => {
  const options = await fixture(t, `process.stdout.write(${JSON.stringify(JSON.stringify(report()))});process.exitCode=1;`);
  await assert.rejects(runEngine(options), /does not match/);
});
test('rejects invalid JSON instead of reporting success', async t => {
  const options = await fixture(t, `process.stdout.write('not JSON');`);
  await assert.rejects(runEngine(options), e => e.code === 'APX_PROTOCOL');
});
test('enforces host deadlines', async t => {
  const options = await fixture(t, 'setInterval(()=>{}, 1000);');
  await assert.rejects(runEngine({ ...options, timeoutMs: 150 }), e => e.code === 'APX_TIMEOUT');
});
test('terminates a cancelled native child', async t => {
  const options = await fixture(t, 'setInterval(()=>{}, 1000);'); const controller = new AbortController();
  const running = runEngine({ ...options, signal: controller.signal }); setTimeout(() => controller.abort(), 100);
  await assert.rejects(running, e => e.code === 'APX_CANCELLED' && e.exitCode === 130);
});
test('does not spawn after cancellation', async () => {
  const controller = new AbortController(); controller.abort();
  await assert.rejects(runEngine({ enginePath: process.execPath, command: 'check', paths: [], signal: controller.signal }), e => e.code === 'APX_CANCELLED');
});
test('bounds native output', async t => {
  const options = await fixture(t, `process.stdout.write('a'.repeat(100000));setInterval(()=>{},1000);`);
  await assert.rejects(runEngine({ ...options, maxOutputBytes: 1000 }), e => e.code === 'APX_OUTPUT_LIMIT');
});
test('does not inherit credentials from the parent environment', async t => {
  const old = process.env.APX_TEST_SECRET; process.env.APX_TEST_SECRET = 'must-not-reach-child';
  t.after(() => { if (old === undefined) delete process.env.APX_TEST_SECRET; else process.env.APX_TEST_SECRET = old; });
  const options = await fixture(t, `const r=${JSON.stringify(report())};r.limitations=[process.env.APX_TEST_SECRET??'absent'];process.stdout.write(JSON.stringify(r));`);
  assert.deepEqual((await runEngine(options)).limitations, ['absent']);
});
test('requires explicit executable paths to be absolute', async () => {
  await assert.rejects(resolveBundledEngine('.', 'apx.exe'), e => e.code === 'APX_ENGINE_PATH');
});
test('missing engine has a diagnostic instead of a fallback download', async () => {
  await assert.rejects(resolveBundledEngine(join(tempRoot, 'missing')), e => e.code === 'APX_ENGINE_MISSING');
});

test('assertion counts must conserve executions and agree with test outcomes', () => {
  const r=report('test'); r.summary.tests=1; r.summary.passed=1;
  r.tests=[{id:'Example.test',path:'Example.cls',line:1,status:'passed',message:'',durationMs:0,assertions:{executed:2,passed:2,failed:0},setupAssertions:{executed:1,passed:1,failed:0}}];
  assert.equal(validateReport(r).tests[0].assertions.executed,2);
  for(const assertions of [{executed:2,passed:1,failed:0},{executed:1.5,passed:1.5,failed:0},{executed:1,passed:0,failed:1},undefined]) {
    assert.throws(()=>validateReport({...r,tests:[{...r.tests[0],assertions}]}),EngineRunError);
  }
  assert.throws(()=>validateReport({...r,tests:[{...r.tests[0],setupAssertions:{executed:1,passed:0,failed:1}}]}),/contradict/);
});

test('input fingerprint binds both manifests and rejects duplicate paths', () => {
  const r=report(); r.inputs=[{path:resolve('resource.png'),sha256:'a'.repeat(64)}]; r.inputFingerprint=inputFingerprint(r.inputs,r.inputDirectories);
  assert.equal(validateReport(r).inputs.length,1);
  assert.throws(()=>validateReport({...r,inputDirectories:r.inputs.map(input=>({...input,includeIgnored:false}))}),/fingerprint/);
  const duplicate={...r,inputs:[...r.inputs,...r.inputs]};duplicate.inputFingerprint=inputFingerprint(duplicate.inputs,duplicate.inputDirectories);
  assert.throws(()=>validateReport(duplicate),/duplicates/);
});

test('consumed binary payload bytes are checked after native execution', async t => {
  const options=await fixture(t,'');
  const file=join(options.projectRoot,'image.png');
  const bytes=Buffer.from([0,255,7,128]);await writeFile(file,bytes);
  const r=report();r.inputs=[{path:file,sha256:createHash('sha256').update(bytes).digest('hex')}];r.inputFingerprint=inputFingerprint(r.inputs,r.inputDirectories);
  await writeFile(join(options.projectRoot,'check'),`process.stdout.write(${JSON.stringify(JSON.stringify(r))});`);
  assert.equal((await runEngine(options)).inputFingerprint,r.inputFingerprint);
  await writeFile(join(options.projectRoot,'check'),`require('node:fs').writeFileSync(${JSON.stringify(file)},'changed');process.stdout.write(${JSON.stringify(JSON.stringify(r))});`);
  await assert.rejects(runEngine(options),e=>e.code==='APX_INPUT_CHANGED');
  await rm(file);
  await assert.rejects(verifyReportInputs(r),e=>e.code==='APX_INPUT_CHANGED');
});

test('directory additions cannot hide behind unchanged consumed file hashes', async t => {
  const options=await fixture(t,'');
  const r=report();r.inputDirectories=[{path:options.projectRoot,sha256:(await directoryFingerprint(options.projectRoot)).sha256,includeIgnored:false}];r.inputFingerprint=inputFingerprint(r.inputs,r.inputDirectories);
  await writeFile(join(options.projectRoot,'check'),`require('node:fs').writeFileSync('added.csv','new');process.stdout.write(${JSON.stringify(JSON.stringify(r))});`);
  await assert.rejects(runEngine(options),e=>e.code==='APX_INPUT_CHANGED');
});

test('directory manifests use UTF8 byte ordering, ignore generated directories and remain bounded', async t => {
  const options=await fixture(t,'');
  await rm(join(options.projectRoot,'check'));
  await mkdir(join(options.projectRoot,'.git'));
  await writeFile(join(options.projectRoot,'\u{10000}'),'a');await writeFile(join(options.projectRoot,'\ue000'),'b');
  const expected=[{name:'\ue000',kind:'file'},{name:'\u{10000}',kind:'file'}];
  assert.equal((await directoryFingerprint(options.projectRoot)).sha256,createHash('sha256').update(JSON.stringify(expected)).digest('hex'));
  assert.equal((await directoryFingerprint(options.projectRoot,100_000,undefined,true)).entries,3,'Expanded resource folders must include names normally ignored by project scanning');
  await assert.rejects(directoryFingerprint(options.projectRoot,1),e=>e.code==='APX_INPUT_LIMIT');
  const abort=new AbortController();abort.abort();
  await assert.rejects(verifyReportInputs(report(),abort.signal),e=>e.code==='APX_CANCELLED');
});
