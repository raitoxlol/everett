#!/usr/bin/env node
import { constants, openSync, closeSync, fstatSync, readFileSync, writeFileSync, lstatSync } from 'node:fs';
import { randomBytes } from 'node:crypto';
import { resolve } from 'node:path';
const usage='node web-mesh/setup.mjs request --credential-file <private cloudflare-auth.json> --origin <console origin> --output <new private env file> [--allow-local]';
async function main() {
 const [command,...args]=process.argv.slice(2);
 if(command!=='request')throw Error(usage);
 const options={};for(let i=0;i<args.length;i++){const key=args[i];if(key==='--allow-local'){options[key]=true;continue;}if(!['--credential-file','--origin','--output'].includes(key)||options[key]||!args[i+1])throw Error(usage);options[key]=args[++i];}
 if(!options['--credential-file']||!options['--origin']||!options['--output'])throw Error(usage);
 const output=resolve(options['--output']);
 try{lstatSync(output);throw Error('Output already exists. Choose a new private file.');}catch(error){if(error.code!=='ENOENT')throw error;}
 const fd=openSync(resolve(options['--credential-file']),constants.O_RDONLY|constants.O_NOFOLLOW);
 let credential;try{const info=fstatSync(fd);if(!info.isFile()||info.size>16384||info.uid!==process.getuid()||(info.mode&0o077)!==0)throw Error('Credential file must be owner-only and under 16 KiB.');credential=JSON.parse(readFileSync(fd,'utf8'));}finally{closeSync(fd);}
 const issuer=new URL(credential.issuer),origin=new URL(options['--origin']);
 const loopback=url=>url.protocol==='http:'&&['127.0.0.1','localhost','[::1]'].includes(url.hostname);
 const local=options['--allow-local']===true&&loopback(issuer);
 if(issuer.origin!==credential.issuer||issuer.username||issuer.password||(issuer.protocol!=='https:'&&!local)||origin.origin!==options['--origin']||(origin.protocol!=='https:'&&!(local&&loopback(origin))))throw Error('Exact HTTPS issuer and console origins are required. Local mode allows loopback only.');
 if(!/^evt_[a-f0-9]{64}$/.test(credential.token)||credential.expires_at<=Date.now())throw Error('A current native credential is required.');
 const response=await fetch(issuer.origin+'/v1/mesh/setup',{method:'POST',headers:{Authorization:'Bearer '+credential.token,'Content-Type':'application/json'},body:JSON.stringify({origin:origin.origin}),redirect:'error',signal:AbortSignal.timeout(10000)});
 if(response.status!==201)throw Error('Setup was denied or unavailable. No console configuration was written.');
 const grant=await response.json();
 if(!/^emr_[a-f0-9]{64}$/.test(grant.token)||grant.scope!=='mesh:read:v1'||grant.owner!==credential.owner||new URL(grant.approvalUrl).origin!==issuer.origin)throw Error('Setup response binding did not match.');
 const config={ENABLED:'false',CONSOLE_ORIGIN:origin.origin,CONSOLE_SECRET:randomBytes(32).toString('hex'),SESSION_KEY:randomBytes(32).toString('hex'),MESH_ENDPOINT:issuer.origin,MESH_TOKEN:grant.token,MESH_OWNER:grant.owner,...(local?{MESH_ALLOW_LOCAL:'true'}:{})};
 const out=openSync(output,constants.O_WRONLY|constants.O_CREAT|constants.O_EXCL|constants.O_NOFOLLOW,0o600);
 try{writeFileSync(out,Object.entries(config).map(([key,value])=>key+'='+JSON.stringify(value)).join('\n')+'\n');}finally{closeSync(out);}
 console.log(`Pending read grant saved privately to ${output}. Console remains disabled.\nOwner approval: ${grant.approvalUrl}\nScope: mesh:read:v1. Expires: ${new Date(grant.expiresAt).toISOString()}.\nAfter approval, set ENABLED=true in the private server environment. No deployment was performed.`);
}
main().catch(error=>{console.error(error.message===usage?usage:'Mesh setup failed. '+(typeof error.message==='string'&&!error.message.includes('emr_')&&!error.message.includes('evt_')&&error.message.length<200?error.message:'Check the private credential and local configuration.'));process.exitCode=1;});
