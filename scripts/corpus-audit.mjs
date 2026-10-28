#!/usr/bin/env node
import fs from 'node:fs/promises';
import path from 'node:path';
import {fileURLToPath} from 'node:url';
import {execFileSync} from 'node:child_process';
import {featuresIn,maskSource,scanTests,sha256,assertConserved} from './corpus-lib.mjs';

const project=path.dirname(path.dirname(fileURLToPath(import.meta.url)));
const args=process.argv.slice(2), options={};
for (let i=0;i<args.length;i+=2) { if (!['--repo','--output'].includes(args[i]) || !args[i+1]) throw new Error('Usage: node scripts/corpus-audit.mjs --repo <pinned clone> [--output <directory>]'); options[args[i].slice(2)]=args[i+1]; }
if (!options.repo) throw new Error('--repo is required.');
const repo=path.resolve(options.repo), output=path.resolve(options.output??path.join(project,'docs'));
const pin=JSON.parse(await fs.readFile(path.join(project,'corpus/apex-recipes.pin.json'),'utf8'));
const git=(...argv)=>execFileSync('git',['-C',repo,...argv],{encoding:'utf8',timeout:30000,maxBuffer:16*1024*1024,windowsHide:true}).trim();
if (git('rev-parse','HEAD')!==pin.revision || git('status','--porcelain','--untracked-files=all')) throw new Error('Feature audit requires the unchanged pinned upstream revision.');
const files=[];
for (const sourceFile of pin.sourceFiles) {
  const source=await fs.readFile(path.join(repo,sourceFile.path),'utf8');
  if (sha256(source.replaceAll('\r\n','\n'))!==sourceFile.sha256) throw new Error(`Source hash mismatch: ${sourceFile.path}`);
  const masked=maskSource(source);
  files.push({path:sourceFile.path,className:/\b(?:class|interface|trigger)\s+(\w+)/i.exec(masked)?.[1],features:featuresIn(source),testMethods:scanTests(source,sourceFile.path),apiCalls:[...new Set([...masked.matchAll(/\b([A-Za-z_]\w*(?:\s*\.\s*[A-Za-z_]\w*)+)\s*\(/g)].map(m=>m[1].replaceAll(/\s/g,'')))].sort(),masked});
}
const classes=new Map(files.filter(f=>f.className).map(f=>[f.className.toLowerCase(),f]));
for (const file of files) file.references=[...new Set([...file.masked.matchAll(/\b[A-Za-z_]\w*\b/g)].map(m=>m[0].toLowerCase()).filter(id=>classes.has(id)&&id!==file.className?.toLowerCase()))].map(id=>classes.get(id).className).sort();
function closure(file) {
  const seen=new Set(), queue=[file];
  while(queue.length) { const next=queue.pop(); if(seen.has(next.path))continue; seen.add(next.path); for(const name of next.references)queue.push(classes.get(name.toLowerCase())); }
  return [...seen].sort();
}
const byPath=new Map(files.map(f=>[f.path,f]));
const tests=files.flatMap(f=>f.testMethods.map(t=>({id:t.id,path:t.path,line:t.line,family:t.family,seeAllData:t.seeAllData,directMethodFeatures:featuresIn(f.masked.slice(t.bodyStart,t.end)).map(x=>x.feature),testClassFeatures:f.features.map(x=>x.feature),referencedClasses:f.references,conservativeDependencyFiles:closure(f)}))).sort((a,b)=>a.id.localeCompare(b.id));
assertConserved(pin.tests,tests,'Feature inventory');
for (const test of tests) test.conservativeDependencyFeatures=[...new Set(test.conservativeDependencyFiles.flatMap(p=>byPath.get(p).features.map(f=>f.feature)))].sort();
const families=[...new Set(tests.map(t=>t.family))].sort().map(family=>{
  const members=tests.filter(t=>t.family===family), paths=[...new Set(members.map(t=>t.path))].sort();
  return {family,tests:members.length,classes:paths.length,testFiles:paths,directFeatures:[...new Set(members.flatMap(t=>t.testClassFeatures))].sort(),conservativeDependencyFeatures:[...new Set(members.flatMap(t=>t.conservativeDependencyFeatures))].sort(),directReferencedClasses:[...new Set(members.flatMap(t=>t.referencedClasses))].sort()};
});
const featureCoverage=[...new Set(files.flatMap(f=>f.features.map(x=>x.feature)))].sort().map(feature=>({feature,sourceFiles:files.filter(f=>f.features.some(x=>x.feature===feature)).map(f=>f.path),directTestMethodCount:tests.filter(t=>t.directMethodFeatures.includes(feature)).length,conservativeTestDependencyCount:tests.filter(t=>t.conservativeDependencyFeatures.includes(feature)).length}));
const tracked=git('ls-files','-z').split('\0').filter(Boolean);
const metadata={};
for(const file of tracked.filter(f=>f.startsWith('force-app/main/default/')&&!/\.(cls|trigger)(?:-meta\.xml)?$/.test(f))) { const kind=file.split('/')[3]; (metadata[kind]??=[]).push(file); }
const report={schemaVersion:1,repository:pin.repository,revision:pin.revision,sourceFiles:files.length,tests:tests.length,methodology:'Independent lexical inventory; comments and strings excluded. Class references and their transitive closure are conservative file-level dependencies, not a runtime call graph or a claim that every feature is reached. Dynamic query strings are intentionally not parsed. Native corpus reports remain authoritative for observed first blockers.',families,featureCoverage,metadata,files:files.map(({masked,testMethods,...file})=>({...file,testIds:testMethods.map(t=>t.id)})),testInventory:tests};
await fs.mkdir(output,{recursive:true});
await fs.writeFile(path.join(output,'corpus-feature-matrix.json'),JSON.stringify(report,null,2)+'\n');
const lines=['# Apex Recipes feature and dependency matrix','',`Pinned revision: \`${pin.revision}\`. ${files.length} Apex files, ${tests.length} independently inventoried test methods.`,'',report.methodology,'','## Test families','','| Family | Tests | Test classes | Features directly present in test classes |','| --- | ---: | ---: | --- |',...families.map(f=>`| ${f.family} | ${f.tests} | ${f.classes} | ${f.directFeatures.join(', ')} |`),'','## Feature reach beyond current first blockers','','The final column is a conservative upper bound from class references. It deliberately includes helper methods and branches that a particular test may not execute. The method column excludes setup/static initialization and called helpers. Neither column replaces runtime coverage.','','| Feature | Source files | Direct test methods | Tests whose dependency closure contains feature |','| --- | ---: | ---: | ---: |',...featureCoverage.map(f=>`| ${f.feature} | ${f.sourceFiles.length} | ${f.directTestMethodCount} | ${f.conservativeTestDependencyCount} |`),'','## Supplied metadata','','| Metadata directory | Files |','| --- | ---: |',...Object.entries(metadata).sort().map(([kind,items])=>`| ${kind} | ${items.length} |`),'','The companion `corpus-feature-matrix.json` contains every source file, feature evidence line, dotted API call, class dependency, test identity, and metadata path. Regenerate using `node scripts/corpus-audit.mjs --repo <unchanged pinned clone>`. See `CORPUS-PLAN.md` for dependency sequencing and configuration requirements.',''];
await fs.writeFile(path.join(output,'CORPUS-FEATURES.md'),lines.join('\n'));
console.log(JSON.stringify({sourceFiles:files.length,tests:tests.length,families:families.map(f=>({family:f.family,tests:f.tests})),output},null,2));
