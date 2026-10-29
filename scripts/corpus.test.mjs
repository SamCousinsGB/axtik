import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import {scanTests,assertConserved,compareBaseline,negativeMutation,entrySentinelMutation,verifiedNegativeControl,captureMutationObservation,validateWeaknessBaseline,compareWeaknessBaseline,sha256,featuresIn} from './corpus-lib.mjs';

// Synthetic protocol observations exercise rejection paths; the integration
// test below separately obtains genuine assertion diagnostics from the engine.
function control(status='failed') {
  const project=path.resolve('synthetic-corpus'),source=`synthetic source ${status}`,file=path.join(project,'Sample.cls');
  const message=status==='failed'?'Assertion Failed: probe':'Passed in experimental local evaluator; no test-body assertions executed';
  const expected={id:'Sample.real',path:'Sample.cls',line:1};
  const report={schemaVersion:2,command:'test',status:status==='failed'?'failed':'preview_passed',exitCode:status==='failed'?1:4,
    tests:[{...expected,path:file,status,message,assertions:{executed:status==='failed'?1:0,passed:0,failed:status==='failed'?1:0},setupAssertions:{executed:0,passed:0,failed:0}}],
    summary:{tests:1,passed:status==='passed'?1:0,failed:status==='failed'?1:0,blocked:0,errored:0},
    diagnostics:status==='failed'?[{code:'APX_ASSERTION',severity:'error',message,path:file,line:2,column:2,endLine:2,endColumn:20}]:[],inputs:[{path:file,sha256:sha256(source)}]};
  return {...captureMutationObservation(report,expected,source,project),kind:'assertion-inversion'};
}

test('independent inventory ignores comments, strings and helper methods but retains SeeAllData and legacy tests', () => {
  const source="@IsTest class Sample { /* @IsTest static void fake(){} */ String label='@IsTest static void fake2(){}'; @IsTest(SeeAllData=true) @SuppressWarnings('anything') static void real() { String text='}'; } @TestSetup static void setup() {} static testMethod void oldStyle() {} static void helper(){} }";
  const tests=scanTests(source,'force-app/tests/Sample.cls');
  assert.deepEqual(tests.map(t=>t.id),['Sample.real','Sample.oldStyle']);
  assert.equal(tests[0].seeAllData,true);
  assert.equal(source[tests[0].end-1],'}');
});

test('identity conservation rejects dropped, substituted and duplicated tests even if total counts match', () => {
  const expected=[{id:'A.one'},{id:'A.two'}];
  assert.throws(()=>assertConserved(expected,[{id:'A.one'}],'test'),/conservation/);
  assert.throws(()=>assertConserved(expected,[{id:'A.one'},{id:'B.other'}],'test'),/conservation/);
  assert.throws(()=>assertConserved(expected,[{id:'A.one'},{id:'A.one'}],'test'),/conservation/);
  assert.doesNotThrow(()=>assertConserved(expected,[{id:'A.two'},{id:'A.one'}],'test'));
});

test('net pass-count gains cannot conceal individual regressions', () => {
  const previous={engineSha256:'old',tests:[{id:'A',status:'passed'},{id:'B',status:'blocked'},{id:'C',status:'blocked'}]};
  const current={tests:[{id:'A',status:'blocked'},{id:'B',status:'passed'},{id:'C',status:'passed'}]};
  const diff=compareBaseline(previous,current);
  assert.deepEqual(diff.gainedPasses,['B','C']);
  assert.deepEqual(diff.regressions,[{id:'A',from:'passed',to:'blocked'}]);
});

test('negative mutations preserve operands and ignore assertion-looking strings and custom calls', () => {
  const source="@IsTest class Sample { @IsTest static void real() { System.Assert.areEqual(5, count()); System.assert(compare('x,y', value), 'message'); Assert.isNotNull(value); custom.areEqual(1,1); String x='System.assert(true)'; } }";
  const result=negativeMutation(source,scanTests(source,'Sample.cls')[0]);
  assert.equal(result.kind,'assertion-inversion');
  assert.match(result.source,/System\.Assert\.areNotEqual\(5, count\(\)\)/);
  assert.ok(result.source.includes("System.assert(!(compare('x,y', value)), 'message')"));
  assert.ok(result.source.includes('Assert.isNull(value)'));
  assert.ok(result.source.includes("custom.areEqual(1,1); String x='System.assert(true)'"));
});

test('tests without inline assertions are explicitly marked as weaker sentinel checks', () => {
  const source='@IsTest class Sample { @IsTest static void real() { helper(); } }';
  const result=negativeMutation(source,scanTests(source,'Sample.cls')[0]);
  assert.equal(result.kind,'entry-sentinel');
  assert.match(result.source,/APX_CORPUS_NEGATIVE_SENTINEL/);
});

test('surviving conditional assertions remain visible beside a separate weaker wiring control', () => {
  const source='@IsTest class Sample { @IsTest static void real() { if (false) { System.assertEquals(1,1); } } }';
  const method=scanTests(source,'Sample.cls')[0];
  const primary=negativeMutation(source,method);
  assert.equal(primary.kind,'assertion-inversion');
  assert.ok(primary.source.includes('System.assertNotEquals(1,1)'));
  assert.ok(!primary.source.includes('APX_CORPUS_NEGATIVE_SENTINEL'));
  const sentinel=entrySentinelMutation(source,method);
  assert.ok(sentinel.source.includes('System.assertEquals(1,1)'));
  assert.ok(sentinel.source.indexOf('APX_CORPUS_NEGATIVE_SENTINEL')<sentinel.source.indexOf('if (false)'));
  const survivor=control('passed');
  assert.equal(verifiedNegativeControl(survivor),false);
  const wiring={...control(),kind:'entry-sentinel-after-survivor'};
  assert.equal(verifiedNegativeControl({...survivor,wiringCheck:wiring}),true);
  assert.equal(survivor.killed,false);
  for(const change of [{status:'blocked'},{exitCode:3},{message:'Engine fault'},{kind:'entry-sentinel'}]) assert.equal(verifiedNegativeControl({...survivor,wiringCheck:{...wiring,...change}}),false);
  assert.equal(verifiedNegativeControl({...survivor,status:'harness-error',wiringCheck:wiring}),false);
});

test('schema-2 mutation witnesses reject assertion words, wrong identities, unrelated errors and missing or contradictory counts',()=>{
  const good=control();assert.equal(verifiedNegativeControl(good),true);
  assert.equal(verifiedNegativeControl({kind:'assertion-inversion',killed:true,status:'failed',exitCode:1,message:'System.DmlException: assertion record missing'}),false);
  for(const change of [
    r=>{delete r.evidenceSchemaVersion;},r=>{delete r.witness;},r=>{r.id='Wrong.test';},r=>{r.witness.tests[0].id='Wrong.test';},
    r=>{r.witness.tests.push(structuredClone(r.witness.tests[0]));},r=>{r.witness.tests[0].path='Other.cls';},r=>{r.witness.tests[0].line=9;},
    r=>{r.witness.summary.failed=0;},r=>{r.witness.status='errored';},r=>{r.witness.exitCode=2;},r=>{r.witness.command='validate';},
    r=>{delete r.witness.tests[0].assertions;},r=>{r.witness.tests[0].assertions.executed=2;},
    r=>{r.witness.tests[0].assertions={executed:0,passed:0,failed:0};r.witness.tests[0].setupAssertions={executed:1,passed:0,failed:1};},
    r=>{r.witness.tests[0].setupAssertions.failed=1;},r=>{r.witness.diagnostics=[];},r=>{r.witness.diagnostics[0].code='APX_EXCEPTION';},
    r=>{r.witness.diagnostics[0].severity='warning';},r=>{r.witness.diagnostics[0].path='Uncaptured.cls';},r=>{r.witness.diagnostics[0].line=0;},
    r=>{r.witness.diagnostics[0].message='different assertion';},r=>{r.witness.diagnostics.push({...r.witness.diagnostics[0],code:'APX_EXCEPTION'});},
    r=>{r.witness.inputs=[];},r=>{r.witness.inputs[0].sha256='b'.repeat(64);},r=>{r.target.sha256='invalid';},
    r=>{r.message=r.witness.tests[0].message=r.witness.diagnostics[0].message='System.DmlException: assertion record missing';r.witness.diagnostics[0].code='APX_EXCEPTION';},
  ]) {const changed=structuredClone(good);change(changed);assert.equal(verifiedNegativeControl(changed),false,JSON.stringify(changed));}
  const helper=structuredClone(good);helper.witness.diagnostics[0].path='Helper.cls';helper.witness.inputs.push({path:'Helper.cls',sha256:'c'.repeat(64)});
  assert.equal(verifiedNegativeControl(helper),true,'one selected test may fail an assertion in a captured helper or async source');
});

test('weakness identities cannot silently grow or be substituted even at unchanged totals',()=>{
  const pin={repository:'repo',revision:'rev',gitTree:'tree',tests:[{id:'Sample.real'},{id:'Other.test'}]};
  const baseline={schemaVersion:1,repository:'repo',revision:'rev',gitTree:'tree',survivingAssertionInversions:['Sample.real'],entrySentinels:['Sample.real'],zeroAssertionTests:['Sample.real']};
  assert.equal(validateWeaknessBaseline(baseline,pin),baseline);
  assert.throws(()=>validateWeaknessBaseline({...baseline,revision:'different'},pin),/different corpus/);
  assert.throws(()=>validateWeaknessBaseline({...baseline,entrySentinels:['Unknown.test']},pin),/Invalid/);
  assert.throws(()=>validateWeaknessBaseline({...baseline,zeroAssertionTests:['Sample.real','Sample.real']},pin),/Invalid/);
  assert.equal(compareWeaknessBaseline(baseline,[],[]).verified,true,'closing an existing gap is allowed');
  for(const [tests,mutations] of [
    [[{id:'Other.test',status:'passed',assertions:{executed:0}}],[]],
    [[],[{id:'Other.test',kind:'entry-sentinel',killed:true}]],
    [[],[{id:'Other.test',kind:'assertion-inversion',killed:false,status:'passed'}]],
  ]) assert.equal(compareWeaknessBaseline(baseline,tests,mutations).verified,false);
});

test('real engine witnesses accept direct and helper assertions and reject exception messages containing assertion',{skip:!process.env.APX_ENGINE_PATH},async t=>{
  // macOS exposes its temporary root through /var -> /private/var. Pass the
  // canonical owned fixture path to the engine's strict input-path checks.
  const project=await fs.realpath(await fs.mkdtemp(path.join(os.tmpdir(),'apx-mutation-witness-')));
  t.after(()=>fs.rm(project,{recursive:true,force:true}));
  const source="@IsTest class Probe {\n@IsTest static void direct() { System.assert(false, 'mutant'); }\n@IsTest static void helper() { Helper.fail(); }\n@IsTest static void misleading() { throw new ProbeException('assertion decoy'); }\n}\nclass ProbeException extends Exception {}\n";
  await fs.writeFile(path.join(project,'Probe.cls'),source);
  await fs.writeFile(path.join(project,'Helper.cls'),"public class Helper { public static void fail() { System.assert(false, 'helper mutant'); } }");
  const {runEngine}=await import('../packages/runner/lib/index.js');
  for(const method of scanTests(source,'Probe.cls')) {
    const report=await runEngine({enginePath:path.resolve(process.env.APX_ENGINE_PATH),command:'test',projectRoot:project,paths:[project],filter:method.id,timeoutMs:30000});
    const result={...captureMutationObservation(report,method,source,project),kind:'assertion-inversion'};
    assert.equal(verifiedNegativeControl(result),method.id!=='Probe.misleading',JSON.stringify(result));
    if(method.id==='Probe.helper') assert.equal(result.witness.diagnostics.find(d=>d.code==='APX_ASSERTION').path,'Helper.cls');
    const wrong=structuredClone(result);wrong.target.sha256='f'.repeat(64);assert.equal(verifiedNegativeControl(wrong),false);
  }
});

test('feature inventory ignores comments and string contents to avoid invented dependencies', () => {
  const found=featuresIn("// Crypto.encrypt(...)\n String note='SELECT Organization'; Map<String, Account> values=new Map<String,Account>();");
  assert.deepEqual(found.map(f=>f.feature),['maps-sets']);
});
