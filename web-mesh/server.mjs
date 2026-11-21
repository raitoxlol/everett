import { createHmac, randomBytes, timingSafeEqual } from 'node:crypto';
import { fixtureSnapshot } from './fixtures/v1.mjs';
import { githubOwner, githubRoutes, readCookie } from './github-auth.mjs';
import { isExactOrigin, readGrantToken } from './grant.mjs';
const headers = { 'Cache-Control':'no-store', 'Content-Type':'application/json; charset=utf-8', 'X-Content-Type-Options':'nosniff', 'Referrer-Policy':'no-referrer' };
const reply = (data,status=200,extra={}) => new Response(data===null?null:JSON.stringify(data),{status,headers:{...headers,...extra}});
const text = (value,max=256) => { if(typeof value!=='string'||!value||value.length>max||/[\x00-\x1f\x7f]/.test(value))throw Error('invalid projection');return value; };
const time = value => { if(!Number.isSafeInteger(value)||value<0)throw Error('invalid timestamp');return value; };
const list = (value,max) => { if(!Array.isArray(value)||value.length>max)throw Error('projection exceeds limit');return value; };
const oneOf = (value,values) => {if(!values.includes(value))throw Error('invalid projection');return value;};
export function adaptSnapshot(value,config) {
  if(value.version!=='everett.mesh.v1'||value.owner!==config.MESH_OWNER||value.consoleOrigin!==config.CONSOLE_ORIGIN)throw Error('projection binding mismatch');
  const source=oneOf(value.source,['cloudflare','fixture']),serverNow=time(value.serverNow);
  const devices=list(value.devices,256).map(d=>({deviceId:text(d.deviceId),name:d.name===null?null:text(d.name,100),lastSeen:d.lastSeen===null?null:time(d.lastSeen),lastSeenConfidence:oneOf(d.lastSeenConfidence,['server-observed-request','unavailable'])}));
  const sessions=list(value.sessions,256).map(s=>{
    const reported=s.selfReported;
    const status=reported?.status===null||reported?.status===undefined?null:oneOf(reported.status,['idle','busy','blocked','offline']);
    return {deviceId:text(s.deviceId),sessionId:text(s.sessionId),harness:text(s.harness,160),lastSeen:time(s.lastSeen),lastSeenConfidence:oneOf(s.lastSeenConfidence,['server-observed-exposure']),selfReported:{status,observedAt:status===null?null:time(reported.observedAt),confidence:status===null?'unavailable':'self-reported'}};
  });
  const receipts=list(value.receipts,100).map(r=>({messageId:text(r.messageId),deviceId:text(r.deviceId),state:oneOf(r.state,['delivered','claimed','replied']),at:time(r.at),confidence:oneOf(r.confidence,['signed-device-observation'])}));
  const cloudAgents = source === 'fixture' && value.cloudAgents
    ? list(value.cloudAgents, 64).map(agent => ({
      agentId: text(agent.agentId, 160),
      name: text(agent.name, 100),
      kind: oneOf(agent.kind, ['codex', 'chat', 'dot']),
    }))
    : null;
  const accountUrl = source === 'cloudflare' && config.MESH_ENDPOINT
    ? new URL('/enroll', config.MESH_ENDPOINT).href
    : null;
  const queued=time(value.queue.queued);
  const truncated={};for(const key of ['devices','sessions','receipts']){if(typeof value.truncated?.[key]!=='boolean')throw Error('invalid projection');truncated[key]=value.truncated[key];}
  return {version:'everett.mesh.v1',source,verification:source==='fixture'?'fixture-only':'cloudflare-read',owner:text(value.owner,2048),serverNow,grantExpiresAt:time(value.grantExpiresAt),fetchedAt:Date.now(),authentication:config.auth ?? 'secret',devices,sessions,cloudAgents,accountUrl,queue:{queued},receipts,truncated};
}
const equal = (a,b) => {const x=Buffer.from(a),y=Buffer.from(b);return x.length===y.length&&timingSafeEqual(x,y);};
function configuration(env) {
  const origin=new URL(env.CONSOLE_ORIGIN);
  if (!isExactOrigin(origin, env.CONSOLE_ORIGIN, true)) throw Error('invalid console origin');
  const auth = env.CONSOLE_AUTH ?? 'secret';
  if (!env.SESSION_KEY || env.SESSION_KEY.length < 32 || !['secret', 'github'].includes(auth)) throw Error('invalid console authentication');
  if (auth === 'secret' && (!env.CONSOLE_SECRET || env.CONSOLE_SECRET.length < 32)) throw Error('private console secret required');
  if(env.MESH_MODE!=='fixture') {
    const endpoint=new URL(env.MESH_ENDPOINT);
    if (!isExactOrigin(endpoint, env.MESH_ENDPOINT, env.MESH_ALLOW_LOCAL === 'true')) throw Error('invalid source');
    if (!readGrantToken.test(env.MESH_TOKEN) || !env.MESH_OWNER) throw Error('read grant required');
  }
  let account = null;
  if (auth === 'github') {
    if (origin.protocol !== 'https:' || env.MESH_MODE === 'fixture' ||
        !env.GITHUB_CLIENT_ID || !env.GITHUB_CLIENT_SECRET) throw Error('GitHub authentication is unconfigured');
    account = githubOwner(env);
  }
  return {...env,auth,githubOwner:account,secure:origin.protocol==='https:'};
}
async function boundedResponse(response,limit=524288) {
  if(!response.body)throw Error('missing projection');
  const reader=response.body.getReader();let size=0,parts=[];
  try {for(;;){const {done,value}=await reader.read();if(done)break;size+=value.byteLength;if(size>limit)throw Error('projection too large');parts.push(value);}return JSON.parse(Buffer.concat(parts).toString('utf8'));}finally{await reader.cancel().catch(()=>{});}
}
export function createConsole(env,sourceFetch=fetch,githubFetch=fetch) {
  return async request => {
    if(env.ENABLED!=='true')return reply({error:'console_disabled'},404);
    let config;try{config=configuration(env);}catch{return reply({error:'console_unconfigured'},503);}
    const url=new URL(request.url);
    if(url.origin!==config.CONSOLE_ORIGIN)return reply({error:'wrong_origin'},403);
    const mac = value => createHmac('sha256',config.SESSION_KEY)
      .update(JSON.stringify([config.CONSOLE_ORIGIN, config.auth, config.MESH_OWNER, value])).digest('hex');
    const cookieName=config.secure?'__Host-everett-console':'everett-console';
    const cookie = (value,age=900) => `${cookieName}=${value}; Path=/; HttpOnly; SameSite=Strict; Max-Age=${age}${config.secure?'; Secure':''}`;
    const issueSession = () => {
      const value = `${Date.now()+900000}.${randomBytes(16).toString('hex')}`;
      return cookie(`${value}.${mac(value)}`);
    };
    if (config.auth === 'github' && ['/api/login', '/api/github-callback'].includes(url.pathname)) {
      if (request.method !== 'GET') return reply({error:'method_not_allowed'},405);
      const signal = AbortSignal.timeout(8000);
      const fetchJson = async (endpoint, options) => {
        const response = await githubFetch(endpoint, {
          ...options, redirect:'error', cache:'no-store', signal,
        });
        if (!response.ok) throw Error('GitHub unavailable');
        return boundedResponse(response, 65536);
      };
      const github = githubRoutes(config, { mac, equal, fetchJson, issueSession, reply });
      return url.pathname === '/api/login' ? github.start() : github.finish(request, url);
    }
    if(url.pathname==='/api/login') {
      if(request.method!=='POST')return reply({error:'method_not_allowed'},405);
      if(request.headers.get('Origin')!==config.CONSOLE_ORIGIN)return reply({error:'wrong_origin'},403);
      if(!request.headers.get('Content-Type')?.startsWith('application/x-www-form-urlencoded'))return reply({error:'invalid_request'},400);
      const body=await request.text();if(body.length>512)return reply({error:'invalid_request'},400);
      const fields=new URLSearchParams(body);
      if([...fields.keys()].join(',')!=='secret'||!equal(fields.get('secret')??'',config.CONSOLE_SECRET))return reply({error:'invalid_login'},401);
      return reply(null,204,{'Set-Cookie':issueSession()});
    }
    const token=readCookie(request,cookieName);
    const match=token.match(/^(\d{13})\.([a-f0-9]{32})\.([a-f0-9]{64})$/);
    if(!match||Number(match[1])<=Date.now()||Number(match[1])>Date.now()+900000||!equal(match[3],mac(`${match[1]}.${match[2]}`)))return reply({error:config.auth==='github'?'github_sign_in_required':'sign_in_required'},401);
    if(url.pathname==='/api/logout') {
      if(request.method!=='POST')return reply({error:'method_not_allowed'},405);
      if(request.headers.get('Origin')!==config.CONSOLE_ORIGIN)return reply({error:'wrong_origin'},403);
      return reply(null,204,{'Set-Cookie':cookie('',0)});
    }
    if(url.pathname!=='/api/mesh')return reply({error:'not_found'},404);
    if(request.method!=='GET')return reply({error:'read_only'},405);
    if(url.search)return reply({error:'invalid_request'},400);
    try {
      if(config.MESH_MODE==='fixture')return reply(adaptSnapshot(fixtureSnapshot(config.MESH_OWNER??'fixture-owner',config.CONSOLE_ORIGIN),{...config,MESH_OWNER:config.MESH_OWNER??'fixture-owner'}));
      const upstream=await sourceFetch(config.MESH_ENDPOINT+'/v1/mesh/snapshot',{headers:{Authorization:'Bearer '+config.MESH_TOKEN,'X-Everett-Console-Origin':config.CONSOLE_ORIGIN},redirect:'error',cache:'no-store',signal:AbortSignal.timeout(8000)});
      if([401,403].includes(upstream.status))return reply({error:'mesh_access_revoked',authentication:config.auth},403);
      if(!upstream.ok)throw Error('source unavailable');
      const data=adaptSnapshot(await boundedResponse(upstream),config);
      if(data.source!=='cloudflare'||Math.abs(Date.now()-data.serverNow)>60000||data.grantExpiresAt<=Date.now())throw Error('stale source');
      return reply(data);
    }catch{return reply({error:'mesh_source_unavailable'},502);}
  };
}
