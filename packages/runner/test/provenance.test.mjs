import test from 'node:test';
import assert from 'node:assert/strict';
import {validateReport, inputFingerprint, describeTestProvenance} from '../lib/index.js';
const ids=['apx.native-method.v1','apx.native-constructor.v1','apx.soql.v1','apx.sosl.v1','apx.dml.v1'];
function report() {
  return {
    schemaVersion:2,engineVersion:'0.2.3',command:'validate',status:'preview_passed',assurance:'experimental-local-execution',exitCode:4,
    projectRoot:'.',files:[],inputs:[],inputDirectories:[],inputFingerprint:inputFingerprint([],[]),diagnostics:[],limitations:[],
    summary:{files:0,selectedFiles:0,tests:1,passed:1,failed:0,blocked:0,errored:0,durationMs:0},
    provenance:{schemaVersion:1,catalogueVersion:'apx-capabilities-v1',conformance:'unqualified',
      compiler:{coverage:'bounded-semantic-subset',conformance:'unqualified'},
      inventory:{scope:'reached-native-dispatch',complete:false,limitations:['Language and internal helper operations are not fully inventoried.']},
      contracts:ids.map(id=>({id,implementation:'partial-local-model',basis:'local-implementation',conformance:'unqualified',independentEvidence:[]}))},
    tests:[{id:'Example.test',path:'Example.cls',line:1,status:'passed',message:'',durationMs:0,assertions:{executed:1,passed:1,failed:0},setupAssertions:{executed:0,passed:0,failed:0},
      provenance:{schemaVersion:1,conformance:'unqualified',inventoryComplete:false,truncated:false,
        operations:[{contractId:ids[0],operation:'system.assertequals',phase:'test',returned:1,failed:0,blocked:0}],
        assumptions:[{id:'explicit-environment-fixture',kind:'synthetic',scope:'configured-for-test'}]}}],
  };
}
test('observed local operations remain unqualified and expose synthetic assumptions',()=>{
  const result=validateReport(report());
  assert.equal(result.exitCode,4);
  assert.match(describeTestProvenance(result.tests[0]),/explicit-environment-fixture/);
  assert.match(describeTestProvenance(result.tests[0]),/Partial.*unqualified/);
});
test('provenance rejects self-certified or incomplete conformance claims',()=>{
  for(const mutate of [
    r=>{r.provenance.conformance='qualified';},
    r=>{r.provenance.compiler.conformance='qualified';},
    r=>{r.provenance.inventory.complete=true;},
    r=>{r.provenance.contracts[0].independentEvidence=['user-supplied'];},
    r=>{r.tests[0].provenance.inventoryComplete=true;},
    r=>{r.tests[0].provenance.conformance='qualified';},
    r=>{delete r.tests[0].provenance;},
    r=>{delete r.provenance;},
    r=>{r.provenance.contracts.push(r.provenance.contracts[0]);},
    r=>{r.provenance.contracts[1]=r.provenance.contracts[0];},
  ]) {const r=report();mutate(r);assert.throws(()=>validateReport(r),/capability inventory/);}
});
test('observations reject unknown contracts, duplicated operations and malformed counters or assumptions',()=>{
  for(const mutate of [
    p=>{p.operations[0].contractId='unreviewed';},
    p=>{p.operations.push(p.operations[0]);},
    p=>{p.operations[0].returned=0;},
    p=>{p.operations[0].failed=-1;},
    p=>{p.operations[0].returned=Number.MAX_SAFE_INTEGER;p.operations[0].failed=1;},
    p=>{p.operations[0].operation='x'.repeat(257);},
    p=>{p.operations[0].operation='method\nforged output';},
    p=>{p.operations[0].phase='oracle';},
    p=>{p.assumptions[0].kind='verified';},
    p=>{p.assumptions[0].id='__proto__';},
    p=>{p.assumptions.push(p.assumptions[0]);},
  ]) {const r=report();mutate(r.tests[0].provenance);assert.throws(()=>validateReport(r),/capability inventory/);}
});
test('truncation is visible and never upgrades the partial inventory',()=>{
  const r=report();r.tests[0].provenance.truncated=true;
  assert.match(describeTestProvenance(validateReport(r).tests[0]),/Partial, truncated/);
});
test('discovery cannot claim reached operations; legacy reports never imply evidence',()=>{
  const r=report();r.command='check';r.status='checked';r.exitCode=0;r.assurance='syntax-and-declarations';r.summary.passed=0;
  r.tests[0].status='not_run';r.tests[0].assertions={executed:0,passed:0,failed:0};
  assert.throws(()=>validateReport(r),/capability inventory/);
  r.tests[0].provenance.operations=[];r.tests[0].provenance.assumptions=[];
  assert.equal(validateReport(r).status,'checked');
  delete r.provenance;delete r.tests[0].provenance;
  assert.match(describeTestProvenance(validateReport(r).tests[0]),/unavailable.*unqualified/);
});
