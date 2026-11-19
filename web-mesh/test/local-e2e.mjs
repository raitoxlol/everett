import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { createServer, request as httpRequest } from 'node:http';
import { createRequire } from 'node:module';
import { mkdtemp,writeFile,readFile,rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { once } from 'node:events';
import { fileURLToPath } from 'node:url';
const root=fileURLToPath(new URL('../../',import.meta.url));
const require=createRequire(new URL('../../cloudflare/package.json',import.meta.url));
const {build}=require(require.resolve('esbuild',{paths:[require.resolve('wrangler/package.json')]}));
const {ed25519,x25519}=require('@noble/curves/ed25519');
const {hkdf}=require('@noble/hashes/hkdf');
const {sha256}=require('@noble/hashes/sha256');
const {chacha20poly1305}=require('@noble/ciphers/chacha');
const bundled=await build({entryPoints:[join(root,'cloudflare/src/protocol.ts')],bundle:true,write:false,format:'esm',platform:'node',logLevel:'silent'});
const wire=await import('data:text/javascript;base64,'+Buffer.from(bundled.outputFiles[0].contents).toString('base64'));
const issuer='http://127.0.0.1:18869',consoleOrigin='http://127.0.0.1:18870',controlPort=18871;
const privateDir=await mkdtemp(join(tmpdir(),'everett-mesh-e2e-'));
const children=[];let control;
async function launch(file,args=[],env={}) {
 const child=spawn(process.execPath,[join(root,file),...args],{cwd:root,env:{...process.env,...env},stdio:['ignore','pipe','pipe']});children.push(child);
 await new Promise((resolve,reject)=>{const timer=setTimeout(()=>reject(Error('local startup timed out')),20000);child.stdout.on('data',data=>{if(/ready:|Private mesh console on/.test(String(data))){clearTimeout(timer);resolve();}});child.once('exit',()=>{clearTimeout(timer);reject(Error('local startup failed'));});child.stderr.on('data',()=>{});});return child;
}
async function cleanup(){if(control)await new Promise(resolve=>control.close(resolve));for(const child of children.reverse()){if(child.exitCode===null){child.kill('SIGTERM');await Promise.race([once(child,'exit'),new Promise(resolve=>setTimeout(resolve,5000))]);if(child.exitCode===null)child.kill('SIGKILL');}}await rm(privateDir,{recursive:true,force:true});}
const cookies=new Map();
async function owner(path,init={}) {
 const headers=new Headers(init.headers);headers.set('Cookie',[...cookies].map(([k,v])=>k+'='+v).join('; '));if(init.method==='POST')headers.set('Origin',issuer);
 const r=await fetch(issuer+path,{...init,headers,redirect:'manual',signal:AbortSignal.timeout(10000)});
 for(const c of r.headers.getSetCookie()){const pair=c.split(';')[0],at=pair.indexOf('=');cookies.set(pair.slice(0,at),pair.slice(at+1));}return r;
}
const json=body=>({method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify(body)});
const form=body=>({method:'POST',headers:{'Content-Type':'application/x-www-form-urlencoded'},body:new URLSearchParams(body)});
async function csrf(path='/enroll'){const r=await owner(path);assert.equal(r.status,200);return (await r.text()).match(/name="csrf" value="([^"]+)"/)[1];}
async function native(token,op,args={}) {const init=json(args);init.headers.Authorization='Bearer '+token;const r=await fetch(issuer+'/v2/relay/'+op,init);assert.equal(r.status,200,'native '+op);return (await r.json()).result;}
async function enroll(name) {
 const pending=await (await fetch(issuer+'/v2/auth/device',json({name}))).json();
 assert.equal((await owner('/enroll/decision',form({csrf:await csrf('/enroll?code='+pending.user_code),user_code:pending.user_code,decision:'approve'}))).status,303);
 const c=await (await fetch(issuer+'/v2/auth/poll',json({device_code:pending.device_code}))).json();assert.equal(c.state,'approved');return c;
}
async function register(c,n) {
 const who=await native(c.token,'whoami'),signing=new Uint8Array(32).fill(n);
 const d={owner:who.owner,deviceId:c.agent_id,signingKey:wire.hexString(ed25519.getPublicKey(signing)),encryptionKey:wire.hexString(x25519.getPublicKey(new Uint8Array(32).fill(n+1))),signature:''};d.signature=wire.hexString(ed25519.sign(wire.deviceBytes(d),signing));await native(c.token,'register',{device:d});return {...c,device:d,signing};
}
try {
 await launch('cloudflare/test/native-fixture.mjs',['serve','18869']);
 const start=await owner('/auth/github?return_to=/enroll'),github=new URL(start.headers.get('Location'));
 assert.equal((await owner('/auth/callback/github?'+new URLSearchParams({state:github.searchParams.get('state'),code:'synthetic-local-only'}))).status,303);
 const a=await register(await enroll('Mac local E2E'),31),b=await register(await enroll('NUC local E2E'),33);
 await native(a.token,'exposeSession',{session:'local-e2e-session',harness:'h'.repeat(160),card:'LOCAL-PRIVATE-CARD'});
 const credentialFile=join(privateDir,'credential.json'),output=join(privateDir,'console.env');
 await writeFile(credentialFile,JSON.stringify({...a,issuer,owner:a.device.owner,expires_at:Date.now()+3600000}),{mode:0o600});
 const setup=spawn(process.execPath,[join(root,'web-mesh/setup.mjs'),'request','--credential-file',credentialFile,'--origin',consoleOrigin,'--output',output,'--allow-local'],{cwd:root,stdio:['ignore','pipe','pipe']});let setupOut='';setup.stdout.on('data',d=>setupOut+=String(d));const [code]=await once(setup,'exit');assert.equal(code,0,'setup CLI');
 const approval=new URL(setupOut.match(/Owner approval: (\S+)/)[1]),grantId=approval.searchParams.get('mesh');
 const config=Object.fromEntries((await readFile(output,'utf8')).trim().split('\n').map(line=>{const at=line.indexOf('=');return [line.slice(0,at),JSON.parse(line.slice(at+1))]}));
 const readSource=()=>fetch(issuer+'/v1/mesh/snapshot',{headers:{Authorization:'Bearer '+config.MESH_TOKEN,'X-Everett-Console-Origin':consoleOrigin}});
 assert.equal(config.ENABLED,'false');assert.equal((await readSource()).status,403);
 assert.equal((await owner('/enroll/mesh',form({csrf:await csrf(approval.pathname+approval.search),id:grantId,decision:'approve'}))).status,303);
 const now=Date.now(),ephemeral=new Uint8Array(32).fill(41);
 const e={owner:a.device.owner,recipientOwner:a.device.owner,grantId:null,fromDevice:a.agent_id,toDevice:b.agent_id,messageId:crypto.randomUUID(),idempotencyKey:crypto.randomUUID(),parent:null,hop:1,createdAt:now,expiresAt:now+3600000,senderFingerprint:wire.fingerprint(a.device),recipientFingerprint:wire.fingerprint(b.device),ephemeralKey:wire.hexString(x25519.getPublicKey(ephemeral)),nonce:'12'.repeat(12),ciphertext:'',signature:''};
 const shared=x25519.getSharedSecret(ephemeral,wire.bytes(b.device.encryptionKey)),key=hkdf(sha256,shared,new TextEncoder().encode('everett-relay-v2'),wire.aadBytes(e),32);
 e.ciphertext=wire.hexString(chacha20poly1305(key,wire.bytes(e.nonce),wire.aadBytes(e)).encrypt(wire.encode({text:'LOCAL-E2E-CONTENT-PRIVATE'})));e.signature=wire.hexString(ed25519.sign(wire.envelopeBytes(e),a.signing));
 await native(a.token,'enqueue',{envelope:e});assert.equal((await (await readSource()).json()).queue.queued,1);
 const pending=await native(b.token,'pending',{deviceId:b.agent_id});assert.equal(pending.envelopes[0].envelope.messageId,e.messageId);
 const o={deviceId:b.agent_id,senderOwner:e.owner,messageId:e.messageId,state:'delivered',at:Date.now(),signature:''};o.signature=wire.hexString(ed25519.sign(wire.observationBytes(e.owner,o),b.signing));await native(b.token,'observe',o);
 const snapshot=await (await readSource()).json();assert.equal(snapshot.queue.queued,0);assert.equal(snapshot.receipts[0].messageId,e.messageId);assert.equal(snapshot.receipts[0].confidence,'signed-device-observation');
 config.ENABLED='true';config.CONSOLE_SECRET='synthetic-local-console-secret-only-20261119';config.PORT='18870';await launch('web-mesh/dev.mjs',[],config);
 const login=await fetch(consoleOrigin+'/api/login',{...form({secret:config.CONSOLE_SECRET}),headers:{'Content-Type':'application/x-www-form-urlencoded',Origin:consoleOrigin}});assert.equal(login.status,204);const sessionCookie=login.headers.get('Set-Cookie').split(';')[0];
 const dashboardRead=()=>fetch(consoleOrigin+'/api/mesh',{headers:{Cookie:sessionCookie}});
 const first=await dashboardRead();assert.equal(first.status,200);const view=await first.json();assert.equal(view.receipts[0].messageId,e.messageId);assert.equal(view.sessions[0].harness,'h'.repeat(160));const text=JSON.stringify(view);for(const hidden of ['LOCAL-PRIVATE-CARD','LOCAL-E2E-CONTENT-PRIVATE',e.ciphertext,a.token,config.MESH_TOKEN])assert.ok(!text.includes(hidden));
 const unexpectedQuery = await fetch(consoleOrigin+'/api/mesh?path=mesh',{headers:{Cookie:sessionCookie}});
 assert.equal(unexpectedQuery.status,400);
 const wrongHostStatus = await new Promise((resolve,reject) => {
  httpRequest(consoleOrigin+'/api/mesh',{headers:{Host:'other.example',Cookie:sessionCookie}},res=>{
   res.resume();
   resolve(res.statusCode);
  }).once('error',reject).end();
 });
 assert.equal(wrongHostStatus,403);
 let revoked=false;
 async function revoke(){assert.equal((await owner('/enroll/mesh',form({csrf:await csrf('/enroll?mesh='+grantId),id:grantId,decision:'revoke'}))).status,303);revoked=true;const denied=await dashboardRead();assert.equal(denied.status,403);assert.ok(!(await denied.text()).includes(e.messageId));}
 console.log('PASS local actual Worker + setup CLI + Vercel handler: default denial, owner approval, encrypted enqueue, signed delivered receipt, content exclusion, same-session reads. GitHub provider is synthetic.');
 if(process.argv.includes('--hold')) {
  control=createServer(async(req,res)=>{if(req.method==='POST'&&req.url==='/revoke'){await revoke();res.writeHead(200);res.end('revoked');}else if(req.method==='GET'&&req.url==='/state'){res.writeHead(200,{'Content-Type':'application/json'});res.end(JSON.stringify({revoked,console:consoleOrigin,messageId:e.messageId}));}else if(req.method==='POST'&&req.url==='/stop'){res.writeHead(200);res.end('stopping');process.emit('SIGTERM');}else{res.writeHead(404);res.end();}}).listen(controlPort,'127.0.0.1');
  console.log('LOCAL BROWSER READY '+consoleOrigin+'; login secret synthetic-local-console-secret-only-20261119; control POST http://127.0.0.1:18871/revoke. No production systems.');
  await new Promise(resolve=>{process.once('SIGTERM',resolve);process.once('SIGINT',resolve)});
 } else {await revoke();console.log('PASS opt-out denies the same existing dashboard session on its next request (403, no receipt data).');
  const logout = await fetch(consoleOrigin+'/api/logout',{method:'POST',headers:{Origin:consoleOrigin,Cookie:sessionCookie}});
  assert.equal(logout.status,204);
  assert.match(logout.headers.get('Set-Cookie'),/Max-Age=0/);}
} finally {await cleanup();}
