/* Run inside a real VS Code Extension Host, not ordinary node --test.
   For installed VSIX testing, point extensionDevelopmentPath at test/driver instead of APX:
   Code --extensionDevelopmentPath=<test/driver> --extensionTestsPath=<this file>
        --disable-workspace-trust --user-data-dir=<isolated> --extensions-dir=<isolated>
   Optional APX_HOST_TEST_REPORT writes machine-readable results for packaging verification.
   APX_HOST_DEMO_PROJECT points at the shipped demo for discovery/preview checks.
   APX_HOST_EXPECT_EXTENSION_ROOT verifies the installed VSIX directory is actually loaded.
   APX_HOST_EXPECT_ENGINE_SHA256 optionally verifies the bundled binary against release evidence.
   No mocked VS Code APIs or mocked engine are used. */
const vscode = require('vscode');
const assert = require('node:assert/strict');
const fs = require('node:fs/promises');
const path = require('node:path');
const os = require('node:os');
const {createHash} = require('node:crypto');
// File watchers are asynchronous. Let the intentional on-disk source changes
// settle before starting a new validation; unsaved edit invalidation is tested
// separately below without a delay.
async function writeSource(file, text) {
  await fs.writeFile(file, text);
  await new Promise(resolve => setTimeout(resolve, 200));
}
async function waitFor(predicate, message) {
  const deadline=Date.now()+5000;
  while (!predicate() && Date.now()<deadline) await new Promise(resolve=>setTimeout(resolve,50));
  assert.ok(predicate(),message);
}

async function run() {
  const results = [];
  let fixture;
  let engineSha256;
  let originalEnvironmentSetting;
  let environmentSettingChanged = false;
  try {
    const extension = vscode.extensions.getExtension('apx-local.apx-local-apex');
    assert.ok(extension, 'APX extension must be installed or under development');
    await extension.activate();
    if (process.env.APX_HOST_EXPECT_EXTENSION_ROOT) {
      assert.equal(await fs.realpath(extension.extensionPath), await fs.realpath(process.env.APX_HOST_EXPECT_EXTENSION_ROOT), 'Tests must load the intended installed VSIX, not a development copy');
    }
    const engine = path.join(extension.extensionPath, 'bin', `${process.platform}-${process.arch}`, process.platform === 'win32' ? 'apx.exe' : 'apx');
    engineSha256 = createHash('sha256').update(await fs.readFile(engine)).digest('hex');
    if (process.env.APX_HOST_EXPECT_ENGINE_SHA256) assert.equal(engineSha256, process.env.APX_HOST_EXPECT_ENGINE_SHA256.toLowerCase(), 'The installed bundled engine must match the tested release binary');
    const commands = new Set(await vscode.commands.getCommands(true));
    for (const command of ['apx.validateSelection', 'apx.validateActiveFile', 'apx.validateFolder', 'apx.validateWorkspace', 'apx.checkSelection', 'apx.refreshTests', 'apx.showOutput', 'apx.showSfInstructions']) assert.ok(commands.has(command), `Missing command ${command}`);
    results.push({name:'commands registered in real Extension Host',passed:true});

    fixture = await fs.mkdtemp(path.join(os.tmpdir(), 'apx-vscode-host-'));
    await fs.writeFile(path.join(fixture, 'sfdx-project.json'), JSON.stringify({packageDirectories:[{path:'force-app',default:true}],sourceApiVersion:'68.0'}));
    const classes = path.join(fixture, 'force-app', 'main', 'default', 'classes');
    await fs.mkdir(classes, {recursive:true});
    const source = path.join(classes, 'Broken.cls');
    const uri = vscode.Uri.file(source);
    await fs.writeFile(source, 'public class Broken { public void broken( }');
    await fs.writeFile(`${source}-meta.xml`, '<?xml version="1.0" encoding="UTF-8"?><ApexClass xmlns="http://soap.sforce.com/2006/04/metadata"><apiVersion>68.0</apiVersion><status>Active</status></ApexClass>');
    await vscode.commands.executeCommand('apx.checkSelection', uri, [uri]);
    const invalid = vscode.languages.getDiagnostics(uri).filter(d => d.source === 'APX');
    assert.ok(invalid.some(d => d.severity === vscode.DiagnosticSeverity.Error), `Native syntax errors must appear in Problems; got ${JSON.stringify(invalid)}`);
    results.push({name:'class command calls native engine and publishes Problems',passed:true});

    await writeSource(source, 'public class Broken { public static Integer twice(Integer value) { return value * 2; } }');
    await vscode.commands.executeCommand('apx.checkSelection', uri, [uri]);
    const repaired = vscode.languages.getDiagnostics(uri).filter(d => d.source === 'APX' && d.severity === vscode.DiagnosticSeverity.Error);
    assert.equal(repaired.length, 0, 'Rechecking repaired source must replace old diagnostics');
    results.push({name:'repaired source clears stale Problems',passed:true});

    await vscode.commands.executeCommand('apx.checkSelection', vscode.Uri.file(classes), [vscode.Uri.file(classes)]);
    assert.equal(vscode.languages.getDiagnostics(uri).filter(d => d.source === 'APX' && d.severity === vscode.DiagnosticSeverity.Error).length, 0);
    results.push({name:'folder command uses the same native pipeline',passed:true});

    await writeSource(source, 'public class Broken { public void broken( }');
    await vscode.commands.executeCommand('apx.checkSelection', uri, [uri]);
    assert.ok(vscode.languages.getDiagnostics(uri).some(d => d.source === 'APX' && d.severity === vscode.DiagnosticSeverity.Error));
    const doc = await vscode.workspace.openTextDocument(uri);
    const editor = await vscode.window.showTextDocument(doc);
    await editor.edit(builder => builder.insert(new vscode.Position(0, 0), '// unsaved edit\n'));
    assert.equal(vscode.languages.getDiagnostics(uri).filter(d => d.source === 'APX').length, 0, 'Unsaved changes invalidate previous diagnostic generation');
    await vscode.commands.executeCommand('workbench.action.files.revert');
    results.push({name:'editing source invalidates applied diagnostics',passed:true});

    if (process.env.APX_HOST_DEMO_PROJECT) {
      const demo = path.resolve(process.env.APX_HOST_DEMO_PROJECT);
      const demoUri = vscode.Uri.file(demo);
      const discovery = await vscode.commands.executeCommand('apx.checkSelection', demoUri, [demoUri]);
      assert.ok(Array.isArray(discovery) && discovery.length === 1, 'Check command must return its applied native report');
      assert.equal(discovery[0].status, 'checked');
      assert.equal(discovery[0].schemaVersion,2);
      assert.match(discovery[0].inputFingerprint,/^[a-f0-9]{64}$/);
      assert.ok(discovery[0].inputs.some(input=>input.path.endsWith('Calculator.cls')));
      assert.equal(discovery[0].summary.tests, 2);
      assert.deepEqual(discovery[0].tests.map(t => t.id).sort(), ['CalculatorTest.evaluatesLoopAndMethod','CalculatorTest.exactDecimalArithmetic']);
      assert.ok(discovery[0].tests.every(t => t.status === 'not_run'), 'Discovery cannot execute tests');
      results.push({name:'bundled engine discovers demo tests through the extension command',passed:true});

      const production = vscode.Uri.file(path.join(demo, 'force-app', 'main', 'default', 'classes', 'Calculator.cls'));
      const validation = await vscode.commands.executeCommand('apx.validateSelection', production, [production]);
      assert.ok(Array.isArray(validation) && validation.length === 1, 'Validate command must return its applied native report');
      assert.equal(validation[0].status, 'preview_passed');
      assert.equal(validation[0].exitCode, 4, 'Successful local execution must remain advisory');
      assert.equal(validation[0].assurance, 'experimental-local-execution');
      assert.equal(validation[0].summary.tests, 2);
      assert.ok(validation[0].tests.every(t => t.status === 'passed'));
      assert.ok(validation[0].tests.every(t => t.assertions.executed>0 && t.assertions.passed===t.assertions.executed && t.assertions.failed===0));
      assert.ok(validation[0].diagnostics.some(d => d.code === 'APX_SELECTION_EXPANDED'));
      assert.ok(validation[0].diagnostics.some(d => d.code === 'APX_PREVIEW' && d.severity === 'warning'));
      results.push({name:'production class validation expands selection and preserves advisory exit 4',passed:true});
    }

    await writeSource(source, 'public class Broken {}');
    const clockSource = path.join(classes, 'ClockTest.cls');
    const clockUri = vscode.Uri.file(clockSource);
    await writeSource(clockSource, '@IsTest private class ClockTest { @IsTest static void fixedClock() { System.assertEquals(Date.newInstance(2001, 2, 3), Date.today()); } }');
    const environment = path.join(fixture, 'local environment.json');
    await writeSource(environment, JSON.stringify({schemaVersion:1,name:'Host integration fixed clock',clock:{now:'2001-02-03T12:00:00Z'},currentUserId:'005000000000001AAA',records:[{object:'User',fields:{Id:'005000000000001AAA',LastName:'Local'}}]}));
    const config = vscode.workspace.getConfiguration('apx');
    originalEnvironmentSetting = config.inspect('environmentPath')?.globalValue;
    environmentSettingChanged = true;
    await config.update('environmentPath', 'local environment.json', vscode.ConfigurationTarget.Global);
    const withEnvironment = await vscode.commands.executeCommand('apx.validateSelection', clockUri, [clockUri]);
    assert.equal(withEnvironment[0].summary.passed, 1, JSON.stringify(withEnvironment));
    assert.equal(withEnvironment[0].exitCode, 4);
    assert.deepEqual(withEnvironment[0].tests[0].assertions,{executed:1,passed:1,failed:0});
    results.push({name:'project-relative environment setting reaches bundled native evaluator',passed:true});

    await writeSource(clockSource, '@IsTest private class ClockTest { @IsTest static void fixedClock() { System.assertEquals(Date.newInstance(2002, 2, 3), Date.today()); } }');
    const changedAssertion = await vscode.commands.executeCommand('apx.validateSelection', clockUri, [clockUri]);
    assert.equal(changedAssertion[0].summary.failed, 1, 'Changing the expected clock must fail through the actual installed extension');
    assert.deepEqual(changedAssertion[0].tests[0].assertions,{executed:1,passed:0,failed:1});
    assert.ok(vscode.languages.getDiagnostics(clockUri).some(d => d.source === 'APX'));
    const environmentDoc = await vscode.workspace.openTextDocument(vscode.Uri.file(environment));
    const environmentEditor = await vscode.window.showTextDocument(environmentDoc);
    await environmentEditor.edit(builder => builder.insert(new vscode.Position(0, 0), ' '));
    assert.equal(vscode.languages.getDiagnostics(clockUri).filter(d => d.source === 'APX').length, 0, 'Unsaved environment changes invalidate the prior result');
    await vscode.commands.executeCommand('workbench.action.files.revert');
    results.push({name:'environment edits invalidate applied diagnostics',passed:true});

    await vscode.commands.executeCommand('apx.validateSelection', clockUri, [clockUri]);
    assert.ok(vscode.languages.getDiagnostics(clockUri).some(d => d.source === 'APX'));
    await config.update('environmentPath', undefined, vscode.ConfigurationTarget.Global);
    assert.equal(vscode.languages.getDiagnostics(clockUri).filter(d => d.source === 'APX').length, 0, 'Environment setting changes invalidate the prior result');
    results.push({name:'environment setting changes invalidate applied diagnostics',passed:true});

    const resources=path.join(fixture,'force-app','main','default','staticresources');
    const bundle=path.join(resources,'Bundle');
    await fs.mkdir(path.join(bundle,'dist'),{recursive:true});
    const resourceXml=type=>`<?xml version="1.0" encoding="UTF-8"?><StaticResource xmlns="http://soap.sforce.com/2006/04/metadata"><cacheControl>Private</cacheControl><contentType>${type}</contentType></StaticResource>`;
    await fs.writeFile(path.join(resources,'Picture.resource-meta.xml'),resourceXml('image/png'));
    const png=path.join(resources,'Picture.png');await fs.writeFile(png,Buffer.from([137,80,78,71,0,255]));
    await fs.writeFile(path.join(resources,'Bundle.resource-meta.xml'),resourceXml('application/zip'));
    const js=path.join(bundle,'script.js');await fs.writeFile(js,'const value=1;');
    const csv=path.join(bundle,'values.csv');await fs.writeFile(csv,'value\n1\n');
    const probe=path.join(classes,'ResourceProbeTest.cls');const probeUri=vscode.Uri.file(probe);
    await writeSource(probe,'@IsTest private class ResourceProbeTest { @IsTest static void failure() { System.assertEquals(1, 2); } }');
    const hasProbeDiagnostics=()=>vscode.languages.getDiagnostics(probeUri).some(d=>d.source==='APX');
    async function applyProbe() {
      await new Promise(resolve=>setTimeout(resolve,250));
      const reports=await vscode.commands.executeCommand('apx.validateSelection',probeUri,[probeUri]);
      assert.equal(reports.length,1,'External project result must be applied before testing its retained watchers');
      assert.equal(reports[0].summary.failed,1);assert.ok(hasProbeDiagnostics());
      return reports[0];
    }
    async function invalidatesAfterMutation(name,mutate) {
      await applyProbe();await mutate();
      await waitFor(()=>!hasProbeDiagnostics(),`${name}: a completed external-project result must be invalidated`);
      results.push({name,passed:true});
    }
    const payloadReport=await applyProbe();
    for(const payload of [png,js,csv]) assert.ok(payloadReport.inputs.some(input=>path.resolve(input.path)===path.resolve(payload)),`Native manifest must include ${payload}`);
    assert.ok(payloadReport.inputDirectories.some(input=>path.resolve(input.path)===path.join(bundle,'dist') && input.includeIgnored===true));
    results.push({name:'real native manifest includes binary and expanded resource inputs',passed:true});
    await invalidatesAfterMutation('completed external result invalidates on binary png payload edit',()=>fs.writeFile(png,Buffer.from([137,80,78,71,1,255])));
    await invalidatesAfterMutation('completed external result invalidates on expanded js payload edit',()=>fs.writeFile(js,'const value=2;'));
    await invalidatesAfterMutation('completed external result invalidates on expanded csv payload edit',()=>fs.writeFile(csv,'value\n2\n'));
    const renamed=path.join(bundle,'renamed.csv');
    await invalidatesAfterMutation('completed external result invalidates on resource payload rename',()=>fs.rename(csv,renamed));
    await invalidatesAfterMutation('completed external result invalidates on resource payload deletion',()=>fs.rm(renamed));
    await invalidatesAfterMutation('expanded resource additions inside normally ignored dist invalidate results',()=>fs.writeFile(path.join(bundle,'dist','new.js'),'const added=true;'));
    await applyProbe();
    const payloadDocument=await vscode.workspace.openTextDocument(vscode.Uri.file(js));
    const payloadEditor=await vscode.window.showTextDocument(payloadDocument);
    await payloadEditor.edit(builder=>builder.insert(new vscode.Position(0,0),'// unsaved resource edit\n'));
    assert.equal(hasProbeDiagnostics(),false,'Unsaved arbitrary resource formats must invalidate the snapshot');
    await vscode.commands.executeCommand('workbench.action.files.revert');
    results.push({name:'unsaved expanded resource edit invalidates applied diagnostics',passed:true});

    const outsideEnvironment=path.join(os.tmpdir(),`${path.basename(fixture)}-environment.json`);
    try {
      await fs.copyFile(environment,outsideEnvironment);
      await config.update('environmentPath',outsideEnvironment,vscode.ConfigurationTarget.Global);
      await invalidatesAfterMutation('completed result retains watcher for environment outside project',async()=>{
        const content=JSON.parse(await fs.readFile(outsideEnvironment,'utf8'));content.name='Changed external fixture';
        await fs.writeFile(outsideEnvironment,JSON.stringify(content));
      });
    } finally { await config.update('environmentPath',undefined,vscode.ConfigurationTarget.Global);await fs.rm(outsideEnvironment,{force:true}); }

    const assertions=path.join(classes,'AssertionProbeTest.cls');const assertionUri=vscode.Uri.file(assertions);
    await writeSource(assertions,'@IsTest private class AssertionProbeTest { @TestSetup static void setup() { System.assert(true); } static void helper() { System.assertEquals(2, 2); } @IsTest static void callsHelper() { helper(); } @IsTest static void noAssertion() { if (false) { System.assert(false); } } }');
    const observations=await vscode.commands.executeCommand('apx.validateSelection',assertionUri,[assertionUri]);
    assert.equal(observations.length,1);assert.equal(observations[0].summary.passed,2);
    const helper=observations[0].tests.find(test=>test.id==='AssertionProbeTest.callsHelper');
    const empty=observations[0].tests.find(test=>test.id==='AssertionProbeTest.noAssertion');
    assert.deepEqual(helper.assertions,{executed:1,passed:1,failed:0});
    assert.deepEqual(empty.assertions,{executed:0,passed:0,failed:0});
    for(const test of [helper,empty]) assert.deepEqual(test.setupAssertions,{executed:1,passed:1,failed:0});
    assert.ok(observations[0].diagnostics.some(d=>d.code==='APX_NO_ASSERTIONS'));
    assert.equal(observations[0].exitCode,4,'Zero assertion completion stays advisory and is explicitly classified');
    results.push({name:'actual assertion invocations distinguish helpers, setup and zero assertion execution',passed:true});

    console.log(`APX Extension Host: ${results.length} tests passed`);
  } catch (error) {
    results.push({name:'host integration failure',passed:false,error:String(error?.stack || error)});
    throw error;
  } finally {
    if (environmentSettingChanged) await vscode.workspace.getConfiguration('apx').update('environmentPath', originalEnvironmentSetting, vscode.ConfigurationTarget.Global);
    if (process.env.APX_HOST_TEST_REPORT) await fs.writeFile(process.env.APX_HOST_TEST_REPORT, JSON.stringify({results,extensionRoot:vscode.extensions.getExtension('apx-local.apx-local-apex')?.extensionPath,engineSha256,extensionHost:process.version,platform:process.platform},null,2));
    // The test creates only this uniquely named directory. Close its editors before cleanup.
    await vscode.commands.executeCommand('workbench.action.closeAllEditors');
    if (fixture) {
      assert.equal(path.dirname(path.resolve(fixture)), path.resolve(os.tmpdir()), 'Cleanup must stay inside the test temporary directory');
      assert.ok(path.basename(fixture).startsWith('apx-vscode-host-'));
      await fs.rm(fixture, {recursive:true,force:true});
    }
  }
}
module.exports = {run};
