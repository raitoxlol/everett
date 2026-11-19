import { createConsole } from '../server.mjs';
export default async function handler(req,res) {
 const headers=new Headers();for(const [k,v]of Object.entries(req.headers))if(v!==undefined)headers.set(k,Array.isArray(v)?v.join(','):v);
 let body;
 if(!['GET','HEAD'].includes(req.method)) {
  const chunks=[];let size=0;
  for await(const chunk of req){size+=chunk.length;if(size>4096){res.writeHead(413,{'Cache-Control':'no-store'});res.end();return;}chunks.push(chunk);}
  body=Buffer.concat(chunks);
 }
 try {
  const request=new Request(new URL(req.url,process.env.CONSOLE_ORIGIN??'https://disabled.invalid'),{method:req.method,headers,body});
  const response=await createConsole(process.env)(request);
  res.writeHead(response.status,Object.fromEntries(response.headers));res.end(Buffer.from(await response.arrayBuffer()));
 }catch{res.writeHead(503,{'Cache-Control':'no-store'});res.end();}
}
