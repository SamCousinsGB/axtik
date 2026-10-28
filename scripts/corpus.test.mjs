import test from 'node:test';
import assert from 'node:assert/strict';
import {scanTests,assertConserved,compareBaseline,negativeMutation,featuresIn} from './corpus-lib.mjs';

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

test('feature inventory ignores comments and string contents to avoid invented dependencies', () => {
  const found=featuresIn("// Crypto.encrypt(...)\n String note='SELECT Organization'; Map<String, Account> values=new Map<String,Account>();");
  assert.deepEqual(found.map(f=>f.feature),['maps-sets']);
});
