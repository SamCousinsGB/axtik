import test from 'node:test';
import assert from 'node:assert/strict';
import * as path from 'node:path';
import { affectsProject, assertionDescription, containedBy, engineFailurePresentation, inputPathKey, needsScopedWatcher, normalizeTargets, RequestLedger, RetainedDisposables, snapshotSelectedTests, summaryLine, testPresentation, type ValidationReport, type TestObservation } from '../src/model';

test('installation recovery distinguishes a broken override from the bundled engine', () => {
  for (const code of ['APX_ENGINE_PATH','APX_ENGINE_MISSING','APX_ENGINE_PERMISSIONS']) {
    const error = Object.assign(new Error('Details remain in Output'), {code});
    const configured = engineFailurePresentation(error, true);
    assert.equal(configured.recovery, 'Open Engine Settings');
    assert.match(configured.message, /Clear apx\.enginePath/);
    const bundled = engineFailurePresentation(error, false);
    assert.equal(bundled.recovery, 'Download Extension');
    assert.match(bundled.message, /no separate engine setup/);
  }
  const unsupported = Object.assign(new Error('Unsupported host linux-arm'), {code:'APX_PLATFORM_UNSUPPORTED'});
  assert.deepEqual(engineFailurePresentation(unsupported, false), {message:unsupported.message,recovery:'View Supported Platforms'});
  const runtime = Object.assign(new Error('Execution timed out'), {code:'APX_TIMEOUT'});
  assert.deepEqual(engineFailurePresentation(runtime, false), {message:runtime.message});
});

test('a preview assertion pass cannot produce a qualified green Test Explorer result', () => {
  const result = {id:'DemoTest.test',path:'DemoTest.cls',line:1,status:'passed',message:'',durationMs:1} as TestObservation;
  assert.equal(testPresentation(result), 'skipped');
  assert.equal(testPresentation({...result,status:'failed'}), 'failed');
  assert.equal(testPresentation({...result,status:'blocked'}), 'errored');
  assert.equal(testPresentation({...result,status:'not_run'}), 'skipped');
});

test('project membership does not confuse sibling prefixes or traversal', () => {
  const root = path.resolve('project');
  assert.ok(containedBy(root, path.join(root, 'force-app', 'Demo.cls')));
  assert.ok(!containedBy(root, `${root}-other/Demo.cls`));
  assert.ok(!containedBy(root, path.join(root, '..', 'outside', 'Demo.cls')));
  assert.deepEqual(normalizeTargets([root, path.join(root, '.')]), [root]);
});

test('older requests cannot publish after a new run or source invalidation', () => {
  const ledger = new RequestLedger();
  const old = ledger.next('project');
  const current = ledger.next('project');
  assert.ok(!ledger.current('project', old));
  assert.ok(ledger.current('project', current));
  ledger.next('project');
  assert.ok(!ledger.current('project', current));
});

test('dependency watcher reconciliation preserves live coverage across repeated validation', () => {
  const retained = new RetainedDisposables();
  const events: string[] = [];
  const subscriptions = new Map<string, {disposed: number; notify(): void; dispose(): void}>();
  const create = (key: string) => {
    assert.equal(subscriptions.has(key),false,'An unchanged watcher must never be recreated');
    events.push(`register:${key}`);
    const subscription = {disposed:0,notify() { if (!this.disposed) events.push(`change:${key}`); },dispose() { this.disposed++; events.push(`dispose:${key}`); }};
    subscriptions.set(key,subscription); return subscription;
  };
  retained.update(['bundle','obsolete'],create);
  const original = subscriptions.get('bundle')!;
  retained.update(['bundle','obsolete','bundle'],create);
  original.notify();
  assert.equal(original.disposed,0);
  assert.equal(events.at(-1),'change:bundle');
  retained.update(['bundle','new-directory'],create);
  assert.ok(events.indexOf('register:new-directory') < events.indexOf('dispose:obsolete'));
  original.notify();
  assert.equal(events.at(-1),'change:bundle','An edit immediately after applying another report keeps its subscription');
  retained.dispose(); retained.dispose();
  for (const subscription of subscriptions.values()) assert.equal(subscription.disposed,1,'Every retired or active watcher is disposed exactly once');
});

test('check summaries distinguish parsing from execution and certification', () => {
  const report = {command:'check',status:'checked',summary:{selectedFiles:3}} as ValidationReport;
  assert.match(summaryLine(report), /Tests were not run/);
  const failedCheck = {...report,status:'failed'} as ValidationReport;
  assert.match(summaryLine(failedCheck), /reported failed/);
  assert.match(summaryLine(failedCheck), /Tests were not run/);
  assert.doesNotMatch(summaryLine(failedCheck), /Experimental/);
  const preview = {...report,command:'validate',status:'preview_passed',tests:[{status:'passed',assertions:{executed:0,passed:0,failed:0},setupAssertions:{executed:2,passed:2,failed:0}}],summary:{selectedFiles:3,tests:1,passed:1,failed:0,blocked:0,errored:0}} as ValidationReport;
  assert.match(summaryLine(preview), /Experimental assurance/);
  assert.match(summaryLine(preview), /no Salesforce certification/);
  assert.match(summaryLine(preview), /0 test assertion\(s\), 1 test\(s\) without assertions/);
  assert.match(assertionDescription(preview.tests[0]), /2 setup assertion\(s\); no test assertion executed/);
  assert.doesNotMatch(summaryLine(preview), /assertion passes/);
});

test('Save and Continue can invalidate discovery without losing the requested test identity', () => {
  const identities = new Map([
    ['one', {id:'ServiceTest.first',path:'/project/ServiceTest.cls',root:'/project'}],
    ['two', {id:'ServiceTest.second',path:'/project/ServiceTest.cls',root:'/project'}],
  ]);
  const selected = snapshotSelectedTests([{id:'one'},{id:'two'}], identities, new Set(['two']));
  identities.clear(); // The save/change listener removes stale discovery entries.
  assert.equal(selected.length, 1);
  assert.equal(selected[0].identity.id, 'ServiceTest.first');
  assert.equal(selected[0].identity.path, '/project/ServiceTest.cls');
  assert.equal(selected[0].item.id, 'one');
});

test('external folders require their own watcher; sibling path prefixes are not workspace coverage', () => {
  const workspace = path.resolve('workspace');
  assert.equal(needsScopedWatcher(path.join(workspace, 'project'), [workspace]), false);
  assert.equal(needsScopedWatcher(workspace, []), true);
  assert.equal(needsScopedWatcher(`${workspace}-external`, [workspace]), true);
  assert.equal(needsScopedWatcher(path.dirname(workspace), [workspace]), true);
});

test('all resource formats and external consumed files affect results; generated project files do not', () => {
  const root=path.resolve('project');
  for(const file of ['logo.png','bundle/script.js','bundle/values.csv']) assert.equal(affectsProject(root,path.join(root,'staticresources',file)),true);
  assert.equal(affectsProject(root,path.join(root,'node_modules','index.js')),false);
  const external=path.resolve('external','environment.json');
  assert.equal(affectsProject(root,external),false);
  assert.equal(affectsProject(root,external,new Set([inputPathKey(external)])),true);
  assert.equal(affectsProject(root,external,new Set(),external),true);
  const resource=path.resolve('external','bundle');
  const observed=new Map([[inputPathKey(resource),true]]);
  assert.equal(affectsProject(root,path.join(resource,'node_modules','new.js'),new Set(),undefined,observed),true);
  assert.equal(affectsProject(root,path.join(resource,'dist'),new Set(),undefined,observed),true);
  const projectDirectory=new Map([[inputPathKey(resource),false]]);
  assert.equal(affectsProject(root,path.join(resource,'new.csv'),new Set(),undefined,projectDirectory),true);
  assert.equal(affectsProject(root,path.join(resource,'node_modules'),new Set(),undefined,projectDirectory),false);
});
