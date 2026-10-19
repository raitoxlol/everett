export function enrollmentPage(cloud: string) {
  return `<!doctype html><html lang="en"><meta charset="utf-8">
<meta name="viewport" content="width=device-width"><title>Everett owner approval</title>
<style>body{font:18px system-ui;max-width:42rem;margin:4rem auto;padding:1rem}button,input{font:inherit;margin:.4rem;padding:.5rem}pre{white-space:pre-wrap}li{margin:1rem 0}</style>
<h1>Everett owner approval</h1>
<p>Only approve a code you started yourself. Compare the full code with your agent's terminal.</p>
<button id="signin">Sign in with GitHub</button><button id="signout">Sign out</button>
<p><label>Device code <input id="code" autocomplete="off" maxlength="12"></label><button id="inspect">Inspect</button></p>
<pre id="details"></pre><button id="approve" disabled>Approve this agent</button><button id="deny" disabled>Deny</button>
<section id="oauth" hidden><h2>Connect an OAuth client</h2>
<p>Approving mints a new, separately revocable agent identity for this client.</p>
<pre id="oauthDetails"></pre><p><label>Agent name <input id="oauthName" maxlength="100" value="ChatGPT dot"></label></p>
<button id="oauthApprove">Approve and mint agent</button><button id="oauthDeny">Deny</button></section>
<h2>Agent credentials</h2><button id="refresh">Refresh list</button><ul id="agents"></ul><p id="status" role="status"></p>
<script>
const cloud = ${JSON.stringify(cloud)};
const $ = id => document.getElementById(id);
let selected = null;
async function api(kind,path,args,authenticated=true) {
  const headers={'Content-Type':'application/json'};
  if(authenticated) {const token=sessionStorage.getItem('ownerToken');if(!token)throw Error('Sign in as the owner first');headers.Authorization='Bearer '+token;}
  const response=await fetch(cloud+'/api/'+kind,{method:'POST',headers,body:JSON.stringify({path,args,format:'json'})});
  const result=await response.json();if(result.status!=='success')throw Error('Request denied or expired. Sign in again if needed.');return result.value;
}
function run(fn){return async()=>{try{$('status').textContent='';await fn();}catch(e){$('status').textContent=e.message;}};}
function reset(){selected=null;$('approve').disabled=true;$('deny').disabled=true;$('details').textContent='';}
$('code').oninput=reset;
$('signin').onclick=run(async()=>{
  sessionStorage.setItem('userCode',$('code').value.trim().toUpperCase());
  const result=await api('action','auth:signIn',{provider:'github',params:{redirectTo:location.origin+'/enroll'}},false);
  sessionStorage.setItem('verifier',result.verifier);location.assign(result.redirect);
});
$('signout').onclick=run(async()=>{try{await api('action','auth:signOut',{});}finally{sessionStorage.clear();reset();$('agents').replaceChildren();$('status').textContent='Signed out';}});
$('inspect').onclick=run(async()=>{
  reset();const code=$('code').value.trim().toUpperCase();const row=await api('query','hosted:inspect',{code});
  $('details').textContent=['Name: '+row.name,'Agent: '+row.agentId,'Code: '+row.code,'State: '+row.state].join(String.fromCharCode(10));
  selected=row.code;$('approve').disabled=row.state!=='pending';$('deny').disabled=row.state!=='pending';
});
async function decide(approve){if(!selected)throw Error('Inspect the code first');await api('mutation','hosted:approve',{code:selected,approve});reset();$('status').textContent=approve?'Agent approved. Return to its terminal.':'Enrollment denied';}
$('approve').onclick=run(()=>decide(true));$('deny').onclick=run(()=>decide(false));
async function showOauth(){
  const id=sessionStorage.getItem('oauthRequest');if(!id||!sessionStorage.getItem('ownerToken'))return;
  const row=await api('query','oauth:inspectRequest',{requestId:id});$('oauth').hidden=false;
  $('oauthDetails').textContent=['Client: '+row.clientName,'Client ID: '+row.clientId,'Redirect: '+row.redirectUri,'Resource: '+row.resource,'Scope: '+row.scope].join(String.fromCharCode(10));
}
async function decideOauth(approve){
  const id=sessionStorage.getItem('oauthRequest');if(!id)throw Error('No OAuth request pending');
  const result=await api('action','oauth:decide',{requestId:id,approve,name:$('oauthName').value});
  sessionStorage.removeItem('oauthRequest');location.assign(result.redirect);
}
$('oauthApprove').onclick=run(()=>decideOauth(true));$('oauthDeny').onclick=run(()=>decideOauth(false));
$('refresh').onclick=run(async()=>{
  const result=await api('query','hosted:credentials',{});$('agents').replaceChildren();
  for(const row of result.credentials){const item=document.createElement('li');item.textContent=row.name+' '+row.agentId+' ['+row.kind+'] ('+row.status+') ';
    if(row.status==='active'){const button=document.createElement('button');button.textContent='Revoke';button.onclick=run(async()=>{if(confirm('Permanently revoke '+row.agentId+'?')){await api('mutation','hosted:revoke',{agentId:row.agentId});button.disabled=true;item.append(' Revoked');}});item.append(button);}
    $('agents').append(item);}
  if(result.truncated)$('agents').append('List truncated to 256 credentials. Use an exact agent ID through the owner API to revoke older entries.');
});
run(async()=>{
  $('code').value=sessionStorage.getItem('userCode')||'';
  const oauthRequest=new URL(location.href).searchParams.get('oauth');
  if(oauthRequest){sessionStorage.setItem('oauthRequest',oauthRequest);history.replaceState(null,'','/enroll');$('status').textContent='Sign in with GitHub to review the OAuth request.';}
  const code=new URL(location.href).searchParams.get('code');
  if(code){const verifier=sessionStorage.getItem('verifier');if(!verifier)throw Error('Missing sign-in verifier. Start sign-in again.');
    history.replaceState(null,'','/enroll');const result=await api('action','auth:signIn',{params:{code},verifier},false);
    sessionStorage.removeItem('verifier');if(!result.tokens?.token)throw Error('Owner sign-in denied');sessionStorage.setItem('ownerToken',result.tokens.token);$('status').textContent='Signed in. Inspect your device code before approval.';}
  await showOauth();
})();
</script></html>`;
}
