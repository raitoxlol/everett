export function fixtureSnapshot(owner='fixture-owner',origin='https://console.example') {
 const now=Date.now();
 return {version:'everett.mesh.v1',source:'fixture',owner,consoleOrigin:origin,serverNow:now,grantExpiresAt:now+86400000,
 devices:[{deviceId:'fixture-mac-01',name:'Mac · fixture',lastSeen:now-19000,lastSeenConfidence:'server-observed-request'},{deviceId:'fixture-nuc-02',name:'NUC · fixture',lastSeen:now-154000,lastSeenConfidence:'server-observed-request'}],
 sessions:[{sessionId:'fixture-session-codex',deviceId:'fixture-mac-01',harness:'codex',lastSeen:now-21000,lastSeenConfidence:'server-observed-exposure',selfReported:{status:'busy',observedAt:now-66000,confidence:'self-reported'}},{sessionId:'fixture-session-claude',deviceId:'fixture-nuc-02',harness:'claude',lastSeen:now-155000,lastSeenConfidence:'server-observed-exposure',selfReported:{status:null,observedAt:null,confidence:'unavailable'}}],
 cloudAgents:[
  {agentId:'fixture-cloud-codex',name:'Demo Codex',kind:'codex'},
  {agentId:'fixture-cloud-chatgpt',name:'Demo ChatGPT',kind:'chat'},
 ],
 queue:{queued:2},receipts:[{messageId:'fixture-message-01',deviceId:'fixture-nuc-02',state:'delivered',at:now-125000,confidence:'signed-device-observation'},{messageId:'fixture-message-02',deviceId:'fixture-mac-01',state:'claimed',at:now-320000,confidence:'signed-device-observation'}],truncated:{devices:false,sessions:false,receipts:false}};
}
