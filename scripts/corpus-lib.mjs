import {createHash} from 'node:crypto';
import path from 'node:path';

export const sha256 = value => createHash('sha256').update(value).digest('hex');
export const portable = value => value.replaceAll('\\', '/');

// Preserve offsets/newlines while removing comments and strings. This deliberately
// does not claim to be an Apex parser: it independently inventories this pinned
// corpus, and the committed inventory is cross-checked against native discovery.
export function maskSource(source) {
  return source.replace(/\/\*[\s\S]*?\*\/|\/\/[^\r\n]*|'(?:\\[\s\S]|[^'\\])*'/g, value => value.replace(/[^\r\n]/g, ' '));
}

export function scanTests(source, file) {
  const masked = maskSource(source);
  const className = /\bclass\s+(\w+)/i.exec(masked)?.[1];
  const tests = [];
  const methods = /((?:@\s*\w+(?:\s*\([^)]*\))?\s*)*)((?:(?:public|private|protected|global|static|final|override|virtual|webservice|testmethod)\s+)*)void\s+(\w+)\s*\(\s*\)\s*\{/gi;
  for (const match of masked.matchAll(methods)) {
    if (!/@\s*istest\b/i.test(match[1]) && !/\btestmethod\b/i.test(match[2])) continue;
    if (!className) throw new Error(`Test method without a class in ${file}`);
    const bodyStart = match.index + match[0].length - 1;
    let depth = 1, end = bodyStart + 1;
    while (end < masked.length && depth) { if (masked[end] === '{') depth++; if (masked[end] === '}') depth--; end++; }
    if (depth) throw new Error(`Unbalanced test method ${file}:${match[3]}`);
    tests.push({id: `${className}.${match[3]}`, path: portable(file), family: portable(file).split('/').slice(0, -1).at(-1), line: source.slice(0, match.index).split('\n').length, start: match.index, bodyStart, end, seeAllData: /seealldata\s*=\s*true/i.test(match[1])});
  }
  return tests;
}

export function assertConserved(expected, actual, label) {
  const ids = actual.map(t => t.id);
  const repeated = ids.filter((id, i) => ids.indexOf(id) !== i);
  const expectedIds = new Set(expected.map(t => t.id));
  const actualIds = new Set(ids);
  const missing = [...expectedIds].filter(id => !actualIds.has(id));
  const added = [...actualIds].filter(id => !expectedIds.has(id));
  if (repeated.length || missing.length || added.length || actual.length !== expected.length) throw new Error(`${label} violates test identity conservation: ${JSON.stringify({expected:expected.length,actual:actual.length,missing,added,repeated})}`);
}

export function compareBaseline(previous, current) {
  assertConserved(previous.tests, current.tests, 'Baseline comparison');
  const before = new Map(previous.tests.map(t => [t.id, t]));
  const transitions = current.tests.filter(t => before.get(t.id).status !== t.status).map(t => ({id:t.id,from:before.get(t.id).status,to:t.status}));
  return {
    previousEngineSha256: previous.engineSha256,
    transitions,
    gainedPasses: transitions.filter(t => t.to === 'passed').map(t => t.id),
    regressions: transitions.filter(t => (t.from === 'passed' && t.to !== 'passed') || (t.from !== 'errored' && t.to === 'errored')),
    blockerChanges: current.tests.filter(t => t.status === 'blocked' && before.get(t.id).status === 'blocked' && t.message !== before.get(t.id).message).map(t => ({id:t.id,from:before.get(t.id).message,to:t.message})),
  };
}

export function blockerGroup(message = '') {
  if (/map_creation|Map\b/i.test(message)) return 'maps';
  if (/static_initializer/i.test(message)) return 'static initializers';
  if (/try_statement|catch|throw_statement|exception/i.test(message)) return 'exceptions';
  if (/nested|inner|constructor|instance|object creation/i.test(message)) return 'object model';
  if (/Crypto|Blob|EncodingUtil/i.test(message)) return 'crypto/blob/encoding';
  if (/query|SOQL|SObject|schema|metadata|DML|insert|update|delete/i.test(message)) return 'schema/database';
  if (/Cache|Organization|UserInfo|Profile|Permission|runAs/i.test(message)) return 'org configuration/security/cache';
  if (/callout|Http|Rest|ConnectApi/i.test(message)) return 'integration mocks';
  return message.replace(/\b\d+\b/g, '#') || 'unspecified';
}

export function summarizeBlockers(tests) {
  const groups = new Map();
  for (const test of tests.filter(t => t.status === 'blocked')) {
    const group = blockerGroup(test.message);
    const entry = groups.get(group) ?? {group,count:0,messages:{},testIds:[]};
    entry.count++; entry.testIds.push(test.id); entry.messages[test.message] = (entry.messages[test.message] ?? 0) + 1; groups.set(group, entry);
  }
  return [...groups.values()].sort((a,b) => b.count-a.count || a.group.localeCompare(b.group));
}

// Invert every supported assertion in a test body, preserving operands and call
// order. This mutates actual upstream expectations, not merely the test name.
// A method with no invertible assertion gets an explicit weaker wiring sentinel.
export function entrySentinelMutation(source, test) {
  const sentinel = "\nSystem.assert(false, 'APX_CORPUS_NEGATIVE_SENTINEL');\n";
  return {kind:'entry-sentinel',source:source.slice(0,test.bodyStart+1)+sentinel+source.slice(test.bodyStart+1),assertions:0};
}

export const mutationEvidenceSchemaVersion = 2;
const object = value => value !== null && typeof value === 'object' && !Array.isArray(value);
const count = value => Number.isSafeInteger(value) && value >= 0;
const position = value => count(value) && value > 0;
const digest = value => typeof value === 'string' && /^[a-f0-9]{64}$/.test(value);
const sourcePath = value => typeof value === 'string' && value.length > 0 && !value.includes('\\') && !value.includes(':') && !value.startsWith('/') && value.split('/').every(part => part && part !== '.' && part !== '..');
const counts = value => object(value) && ['executed','passed','failed'].every(key => count(value[key])) && value.executed === value.passed + value.failed;

// Preserve the native observations that establish a control. A single selected
// test binds helper/async diagnostics without assuming their source is the test
// declaration. The consumed mutated source digest also binds the actual input.
export function captureMutationObservation(observed, expected, source, projectRoot) {
  const relative = value => value ? portable(path.relative(projectRoot,value)) : '';
  const clean = value => typeof value === 'string' ? value.replaceAll(projectRoot,'${MUTANT}') : value;
  const target = {id:expected.id,path:portable(expected.path),line:expected.line,sha256:sha256(source)};
  const tests = observed.tests.map(test => ({id:test.id,path:relative(test.path),line:test.line,status:test.status,message:clean(test.message),assertions:test.assertions,setupAssertions:test.setupAssertions,...(test.provenance ? {provenance:test.provenance} : {})}));
  const diagnostics = observed.diagnostics.map(d => ({...d,path:relative(d.path),message:clean(d.message)}));
  const relevant = new Set([target.path,...diagnostics.filter(d => d.severity === 'error').map(d => d.path)]);
  const inputs = observed.inputs.map(input => ({path:relative(input.path),sha256:input.sha256})).filter(input => relevant.has(input.path));
  const witness = {schemaVersion:observed.schemaVersion,command:observed.command,status:observed.status,exitCode:observed.exitCode,...(observed.provenance ? {provenance:observed.provenance} : {}),
    summary:Object.fromEntries(['tests','passed','failed','blocked','errored'].map(key => [key,observed.summary[key]])),tests,diagnostics,inputs};
  const result = {id:expected.id,evidenceSchemaVersion:mutationEvidenceSchemaVersion,target,witness,status:tests[0]?.status,exitCode:observed.exitCode,message:tests[0]?.message};
  result.killed = verifiedMutationObservation(result,'failed');
  return result;
}

export function verifiedMutationObservation(result, status) {
  if (!object(result) || result.evidenceSchemaVersion !== mutationEvidenceSchemaVersion || !object(result.target) || !object(result.witness)) return false;
  const {target,witness:w} = result;
  if (typeof result.id !== 'string' || target.id !== result.id || !sourcePath(target.path) || !position(target.line) || !digest(target.sha256) ||
      w.schemaVersion !== 2 || w.command !== 'test' || !Array.isArray(w.tests) || w.tests.length !== 1 || !object(w.tests[0]) || !Array.isArray(w.diagnostics) || !Array.isArray(w.inputs) || !object(w.summary)) return false;
  const t = w.tests[0], failure = status === 'failed', exitCode = failure ? 1 : 4;
  if (!['passed','failed'].includes(status) || t.id !== target.id || t.path !== target.path || t.line !== target.line || t.status !== status ||
      typeof t.message !== 'string' || result.status !== t.status || result.message !== t.message || result.exitCode !== exitCode || w.exitCode !== exitCode ||
      w.status !== (failure ? 'failed' : 'preview_passed') || !counts(t.assertions) || !counts(t.setupAssertions) ||
      (failure ? t.assertions.failed < 1 : t.assertions.failed !== 0 || t.setupAssertions.failed !== 0)) return false;
  for (const [key,value] of Object.entries({tests:1,passed:failure?0:1,failed:failure?1:0,blocked:0,errored:0})) if (w.summary[key] !== value) return false;
  if (!w.inputs.every(input => object(input) && sourcePath(input.path) && digest(input.sha256)) || new Set(w.inputs.map(input => input.path)).size !== w.inputs.length ||
      !w.inputs.some(input => input.path === target.path && input.sha256 === target.sha256)) return false;
  if (!w.diagnostics.every(d => object(d) && ['error','warning','information'].includes(d.severity))) return false;
  const errors = w.diagnostics.filter(d => d.severity === 'error');
  if (!failure) return errors.length === 0;
  if (errors.length !== 1) return false;
  const d = errors[0];
  return d.code === 'APX_ASSERTION' && d.message === t.message && sourcePath(d.path) && w.inputs.some(input => input.path === d.path) &&
    [d.line,d.column,d.endLine,d.endColumn].every(position) && (d.endLine > d.line || d.endLine === d.line && d.endColumn >= d.column);
}

export function verifiedNegativeControl(result) {
  if (!object(result) || !['assertion-inversion','entry-sentinel'].includes(result.kind)) return false;
  if (result.killed === true) return !result.wiringCheck && verifiedMutationObservation(result,'failed');
  const wiring = result.wiringCheck;
  return result.kind === 'assertion-inversion' && result.killed === false && verifiedMutationObservation(result,'passed') &&
    wiring?.kind === 'entry-sentinel-after-survivor' && wiring.id === result.id && wiring.target?.path === result.target.path && wiring.target?.line === result.target.line && wiring.target?.sha256 !== result.target.sha256 &&
    wiring.killed === true && verifiedMutationObservation(wiring,'failed');
}

export function validateWeaknessBaseline(baseline,pin) {
  if (!object(baseline) || baseline.schemaVersion !== 1 || ['repository','revision','gitTree'].some(key => baseline[key] !== pin[key])) throw new Error('Weakness baseline belongs to a different corpus.');
  const identities = new Set(pin.tests.map(test => test.id));
  for (const key of ['survivingAssertionInversions','entrySentinels','zeroAssertionTests']) {
    const values = baseline[key];
    if (!Array.isArray(values) || new Set(values).size !== values.length || values.some(id => !identities.has(id))) throw new Error(`Invalid weakness baseline ${key}.`);
  }
  return baseline;
}

export function compareWeaknessBaseline(baseline,tests,mutations) {
  const actual = {survivingAssertionInversions:mutations.filter(m => m.kind === 'assertion-inversion' && m.killed === false && m.status === 'passed').map(m => m.id),
    entrySentinels:mutations.filter(m => m.kind === 'entry-sentinel').map(m => m.id),zeroAssertionTests:tests.filter(t => t.status === 'passed' && t.assertions?.executed === 0).map(t => t.id)};
  const added = Object.fromEntries(Object.entries(actual).map(([key,ids]) => [key,ids.filter(id => !baseline[key].includes(id)).sort()]));
  return {verified:Object.values(added).every(ids => ids.length === 0),added};
}

export function negativeMutation(source, test) {
  const body = source.slice(test.bodyStart + 1, test.end - 1);
  const masked = maskSource(body);
  const swaps = {areequal:'areNotEqual',arenotequal:'areEqual',istrue:'isFalse',isfalse:'isTrue',isnull:'isNotNull',isnotnull:'isNull',assertequals:'assertNotEquals',assertnotequals:'assertEquals'};
  const edits = [];
  const calls = /\b(?:(?:System\s*\.\s*)?Assert\s*\.\s*(areEqual|areNotEqual|isTrue|isFalse|isNull|isNotNull)|System\s*\.\s*(assertEquals|assertNotEquals|assert))\s*\(/gi;
  for (const match of masked.matchAll(calls)) {
    const spelling = match[1] ?? match[2];
    const name = spelling.toLowerCase();
    const nameStart = match.index + match[0].toLowerCase().lastIndexOf(name);
    if (swaps[name]) edits.push({start:nameStart,end:nameStart+spelling.length,text:swaps[name]});
    else {
      const argStart = match.index + match[0].length;
      let depth = 0, end = argStart;
      for (; end < masked.length; end++) {
        const ch = masked[end];
        if ('([{'.includes(ch)) depth++;
        else if (')]}'.includes(ch)) { if (depth === 0) break; depth--; }
        else if (ch === ',' && depth === 0) break;
      }
      edits.push({start:argStart,end:argStart,text:'!('}, {start:end,end,text:')'});
    }
  }
  let mutated = body;
  for (const edit of edits.sort((a,b) => b.start-a.start)) mutated = mutated.slice(0,edit.start)+edit.text+mutated.slice(edit.end);
  if (!edits.length) return entrySentinelMutation(source,test);
  return {kind:'assertion-inversion',source:source.slice(0,test.bodyStart+1)+mutated+source.slice(test.end-1),assertions:edits.length};
}

export const featureRules = [
  ['maps-sets', /\b(?:Map|Set)\s*</gi], ['lists-iteration', /\b(?:List|Iterable|Iterator)\s*<|\bfor\s*\(/gi],
  ['exceptions', /\b(?:try|catch|throw|finally)\b/gi], ['static-initializers', /\bstatic\s*\{/gi],
  ['properties', /\b(?:get|set)\s*(?:;|\{)/gi], ['inheritance-interfaces', /\b(?:extends|implements|override|virtual|abstract|interface)\b/gi],
  ['soql', /\[\s*SELECT\b|\bDatabase\s*\.\s*(?:query|countQuery|getQueryLocator)\b/gi],
  ['relationship-queries', /\(\s*SELECT\b|\b\w+__r\b/gi], ['soql-aggregates', /\b(?:COUNT|SUM|AVG|GROUP\s+BY|HAVING)\s*(?:\(|\w)/gi],
  ['dml', /\b(?:insert|update|upsert|delete|undelete|merge)\s+(?:as\s+(?:user|system)\s+)?\w|\bDatabase\s*\.\s*(?:insert|update|upsert|delete|undelete|merge|setSavepoint|rollback)\b/gi],
  ['schema-reflection', /\bSchema\b|\b(?:getDescribe|describeSObjects|describeSObjectResult|getSObjectType|SObjectType|fields\.getMap)\b/gi],
  ['security-user-mode', /\b(?:WITH\s+USER_MODE|WITH\s+SECURITY_ENFORCED|USER_MODE|stripInaccessible|runAs|calculatePermissionSetGroup|PermissionSet|PermissionSetAssignment|Profile)\b/gi],
  ['metadata-data', /\b\w+__mdt\b|\b(?:ApexClass|EntityDefinition|FieldDefinition)\b/gi],
  ['crypto-blob-encoding', /\b(?:Crypto|Blob|EncodingUtil)\b/gi], ['json-xml', /\b(?:JSON|JSONParser|JSONGenerator|Dom|XmlStreamReader|XmlStreamWriter)\b/gi],
  ['dataweave', /\b(?:DataWeave|DataWeaveScriptResource|DataWeaveScriptException)\b/gi],
  ['sorting-comparators', /\b(?:Comparator|Comparable|compareTo)\b|\.\s*sort\s*\(/gi],
  ['enum-request-context', /\b(?:enum|Quiddity|LoggingLevel)\b|\bRequest\s*\.\s*getCurrent\b/gi],
  ['safe-navigation', /\?\.|\?\?/g],
  ['http-rest-mocks', /\b(?:Http|HttpRequest|HttpResponse|RestContext|RestRequest|RestResponse|HttpCalloutMock|setMock|WebServiceMock)\b/gi],
  ['stub-provider-connectapi', /\b(?:StubProvider|createStub|ConnectApi)\b/gi],
  ['async-jobs', /\b(?:Queueable|QueueableContext|Batchable|Schedulable|enqueueJob|executeBatch|schedule|AsyncApexJob|future)\b/gi],
  ['events-triggers', /\b(?:Trigger|EventBus|publish|SObjectEventBus)\b|\b\w+__e\b/gi],
  ['email', /\bMessaging\b/gi], ['files-content', /\b(?:ContentVersion|ContentDocument|ContentDocumentLink|ContentDistribution)\b/gi],
  ['cache-org-context', /\b(?:Cache|Organization|UserInfo|DatedConversionRate)\b/gi],
  ['dates-times', /\b(?:Date|Datetime|Time)\b/gi], ['type-reflection', /\bType\s*\.|\b(?:forName|newInstance|instanceof)\b/gi],
];

export function featuresIn(source) {
  const masked = maskSource(source);
  return featureRules.flatMap(([feature,pattern]) => {
    const matches = [...masked.matchAll(pattern)];
    return matches.length ? [{feature,lines:[...new Set(matches.map(m => masked.slice(0,m.index).split('\n').length))]}] : [];
  });
}
