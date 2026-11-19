import { createServer } from 'node:http';
import { readFile } from 'node:fs/promises';
import handler from './api/handler.mjs';
const port=Number(process.env.PORT??8788);
if(!process.env.CONSOLE_ORIGIN)process.env.CONSOLE_ORIGIN=`http://127.0.0.1:${port}`;
const files={'/':'index.html','/app.js':'app.js','/style.css':'style.css'};
const types={'.html':'text/html; charset=utf-8','.js':'text/javascript; charset=utf-8','.css':'text/css; charset=utf-8'};
createServer(async(req,res)=>{
 const path=new URL(req.url,'http://127.0.0.1').pathname;
 if(path.startsWith('/api/'))return handler(req,res);
 if(!files[path]){res.writeHead(404);res.end();return;}
 try{const file=files[path];res.writeHead(200,{'Content-Type':types[file.slice(file.lastIndexOf('.'))],'Cache-Control':'no-store','Content-Security-Policy':"default-src 'none'; script-src 'self'; style-src 'self'; connect-src 'self'; form-action 'self'; frame-ancestors 'none'; base-uri 'none'"});res.end(await readFile(new URL('./public/'+file,import.meta.url)));}catch{res.writeHead(503);res.end();}
}).listen(port,'127.0.0.1',()=>process.stdout.write(`Private mesh console on http://127.0.0.1:${port}\n`));
