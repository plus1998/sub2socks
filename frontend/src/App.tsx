import {FormEvent,ReactNode,useEffect,useRef,useState} from 'react';
import {Activity,BookOpen,ChevronRight,Globe2,LogOut,Menu,Moon,Network,Play,RefreshCw,Server,Settings2,ShieldCheck,Square,Sun,Users,X} from 'lucide-react';

type Lang='zh-CN'|'en';
type Item={id:number;name:string;enabled?:boolean;[key:string]:any};
type NodeTestJob={job_id:string;status:'running'|'completed';total:number;done:number;ok:number;failed:number};
type NodeTestResponse={node_id:number;tested_at:number;ok:boolean;latency_ms:number|null;error:string|null};

const text={
  'zh-CN':{overview:'概览',subs:'订阅',nodes:'节点',accounts:'Socks 账号',config:'Mihomo 配置',refresh:'刷新',start:'启动',stop:'停止',logout:'退出',running:'运行中',stopped:'已停止',port:'SOCKS5 端口',addSub:'添加并同步',url:'订阅 URL',name:'名称',optional:'可选',sync:'同步',del:'删除',edit:'编辑',enabled:'启用',type:'类型',server:'服务器',subscription:'订阅',noSubscription:'未关联订阅',unknownSubscription:'未知订阅',user:'用户名',password:'密码',node:'节点',save:'保存',cancel:'取消',preview:'预览配置',empty:'暂无数据',setup:'初始化管理器',setupDesc:'设置管理员账号以开始使用。',login:'管理员登录',loginDesc:'输入管理员账号和密码。',loginFailed:'登录失败：',setupFailed:'初始化失败：',admin:'管理员用户',welcome:'代理服务控制台',status:'服务状态',quick:'集中管理订阅、节点与访问账号',lastSync:'最近同步',signIn:'登录',test:'检测',testAll:'一键检测',testing:'检测中',untested:'未检测',available:'可用',unavailable:'不可用',lastTest:'上次检测',latency:'延迟',testProgress:'检测进度',testSucceeded:'成功',testFailed:'失败',testComplete:'节点检测完成',testError:'节点检测失败：'},
  en:{overview:'Overview',subs:'Subscriptions',nodes:'Nodes',accounts:'Socks Accounts',config:'Mihomo Config',refresh:'Refresh',start:'Start',stop:'Stop',logout:'Sign out',running:'Running',stopped:'Stopped',port:'SOCKS5 Port',addSub:'Add & Sync',url:'Subscription URL',name:'Name',optional:'Optional',sync:'Sync',del:'Delete',edit:'Edit',enabled:'Enabled',type:'Type',server:'Server',subscription:'Subscription',noSubscription:'No subscription',unknownSubscription:'Unknown subscription',user:'Username',password:'Password',node:'Node',save:'Save',cancel:'Cancel',preview:'Preview config',empty:'No data yet',setup:'Initialize manager',setupDesc:'Set an administrator account to get started.',login:'Administrator sign in',loginDesc:'Enter your administrator credentials.',loginFailed:'Sign-in failed: ',setupFailed:'Initialization failed: ',admin:'Admin user',welcome:'Proxy service console',status:'Service status',quick:'Manage subscriptions, nodes and access accounts',lastSync:'Last sync',signIn:'Sign in',test:'Test',testAll:'Test all',testing:'Testing',untested:'Not tested',available:'Available',unavailable:'Unavailable',lastTest:'Last test',latency:'Latency',testProgress:'Test progress',testSucceeded:'Succeeded',testFailed:'Failed',testComplete:'Node tests completed',testError:'Node test failed: '}
};

async function api(path:string,options:RequestInit={}){
  const response=await fetch(path,{...options,headers:{'content-type':'application/json',...options.headers}});
  const raw=await response.text();
  let data:unknown=null;
  try{data=raw?JSON.parse(raw):null}catch{data=raw}
  if(!response.ok){
    const apiError=typeof data==='object'&&data!==null&&'error' in data&&typeof data.error==='string'?data.error:'';
    const message=apiError.trim()?apiError:typeof data==='string'&&data.trim()?data:`Request failed (${response.status}${response.statusText?` ${response.statusText}`:''})`;
    throw new Error(message);
  }
  return data as any;
}
const body=(value:unknown):RequestInit=>({method:'POST',body:JSON.stringify(value)});
const safeError=(value:unknown)=>String(value??'')
  .replace(/([?&](?:password|passwd|pwd|raw)=)[^&\s]*/gi,'$1[redacted]')
  .replace(/\b(password|passwd|pwd|raw)\s*[:=]\s*[^,;\s]+/gi,'$1=[redacted]')
  .replace(/(\w+:\/\/)[^/@\s]+:[^/@\s]+@/g,'$1[redacted]@');

export default function App(){
  const[lang,setLang]=useState<Lang>('zh-CN'),t=text[lang];
  const[dark,setDark]=useState(()=>localStorage.theme!=='light');
  const[mobile,setMobile]=useState(false);
  const[view,setView]=useState<'loading'|'setup'|'login'|'app'>('loading');
  const[active,setActive]=useState('overview');
  const[subs,setSubs]=useState<Item[]>([]),[nodes,setNodes]=useState<Item[]>([]),[accounts,setAccounts]=useState<Item[]>([]),[editing,setEditing]=useState<Item|null>(null);
  const[mihomo,setMihomo]=useState<Record<string,unknown>>({}),[port,setPort]=useState<string|number>('-'),[toast,setToast]=useState(''),[authError,setAuthError]=useState(''),[busy,setBusy]=useState(false),[yaml,setYaml]=useState('');
  const[testingNodes,setTestingNodes]=useState<Set<number>>(()=>new Set());
  const[testJob,setTestJob]=useState<NodeTestJob|null>(null);
  const pollTimer=useRef<number|null>(null);
  const pollController=useRef<AbortController|null>(null);
  const activeNodeTests=useRef<Set<number>>(new Set());
  const mounted=useRef(true);

  useEffect(()=>{document.documentElement.classList.toggle('dark',dark);localStorage.theme=dark?'dark':'light'},[dark]);
  useEffect(()=>()=>{mounted.current=false;if(pollTimer.current!==null)window.clearTimeout(pollTimer.current);pollController.current?.abort()},[]);
  const notify=(message:string)=>{if(!mounted.current)return;setToast(message);window.setTimeout(()=>{if(mounted.current)setToast('')},2600)};
  const refreshNodes=async()=>{const result=await api('/api/nodes');if(mounted.current)setNodes(Array.isArray(result?.nodes)?result.nodes:[])};
  async function load(){
    const status=await api('/api/status');
    setMihomo(status.mihomo||{});setPort(status.socks_port??'-');
    if(!status.initialized)return setView('setup');
    if(!status.authenticated)return setView('login');
    const[subscriptions,nodeResult,socksAccounts]=await Promise.all([api('/api/subscriptions'),api('/api/nodes'),api('/api/socks-accounts')]);
    setSubs(Array.isArray(subscriptions)?subscriptions:[]);setNodes(Array.isArray(nodeResult?.nodes)?nodeResult.nodes:[]);setAccounts(Array.isArray(socksAccounts)?socksAccounts:[]);setView('app');
  }
  useEffect(()=>{load().catch(error=>{notify(safeError(error instanceof Error?error.message:error));setView('login')})},[]);
  async function action(fn:()=>Promise<void>){setBusy(true);try{await fn()}catch(error){notify(safeError(error instanceof Error?error.message:error))}finally{if(mounted.current)setBusy(false)}}
  const auth=async(event:FormEvent<HTMLFormElement>,path:string)=>{
    event.preventDefault();setAuthError('');setBusy(true);
    try{await api(path,body(Object.fromEntries(new FormData(event.currentTarget))))}
    catch(error){const prefix=view==='setup'?t.setupFailed:t.loginFailed;setAuthError(prefix+safeError(error instanceof Error?error.message:error));setBusy(false);return}
    setView('app');try{await load()}catch(error){notify(safeError(error instanceof Error?error.message:error))}finally{setBusy(false)}
  };
  const nav=[['overview',Activity,t.overview],['subscriptions',BookOpen,t.subs],['nodes',Network,t.nodes],['accounts',Users,t.accounts],['config',Settings2,t.config]] as const;

  const testNode=async(id:number)=>{
    if(testJob||activeNodeTests.current.has(id))return;
    activeNodeTests.current.add(id);
    setTestingNodes(current=>new Set(current).add(id));
    try{
      const result=await api(`/api/nodes/${id}/test`,body({})) as NodeTestResponse;
      if(mounted.current)setNodes(current=>current.map(node=>node.id===id?{...node,last_tested_at:result.tested_at,last_test_ok:result.ok,last_test_latency_ms:result.latency_ms,last_test_error:result.error}:node));
      await refreshNodes();
    }catch(error){notify(t.testError+safeError(error instanceof Error?error.message:error))}
    finally{activeNodeTests.current.delete(id);if(mounted.current)setTestingNodes(current=>{const next=new Set(current);next.delete(id);return next})}
  };
  const pollNodeTests=(jobId:string)=>{
    pollController.current?.abort();
    const controller=new AbortController();pollController.current=controller;
    const poll=async()=>{
      try{
        const job=await api(`/api/node-tests/${encodeURIComponent(jobId)}`,{signal:controller.signal}) as NodeTestJob;
        if(!mounted.current||controller.signal.aborted)return;
        setTestJob(job);
        if(job.status==='completed'||job.done>=job.total){
          pollController.current=null;
          await refreshNodes();
          if(mounted.current){setTestJob(null);notify(`${t.testComplete}: ${t.testSucceeded} ${job.ok}, ${t.testFailed} ${job.failed}`)}
          return;
        }
        pollTimer.current=window.setTimeout(poll,1000);
      }catch(error){
        if(controller.signal.aborted)return;
        pollController.current=null;
        if(mounted.current){setTestJob(null);notify(t.testError+safeError(error instanceof Error?error.message:error))}
      }
    };
    void poll();
  };
  const testAll=async()=>{
    if(testJob||activeNodeTests.current.size>0)return;
    try{
      const job=await api('/api/nodes/test-all',body({})) as NodeTestJob;
      if(!mounted.current)return;
      setTestJob(job);
      if(job.status==='completed'||job.done>=job.total){await refreshNodes();if(mounted.current){setTestJob(null);notify(`${t.testComplete}: ${t.testSucceeded} ${job.ok}, ${t.testFailed} ${job.failed}`)}}
      else pollNodeTests(job.job_id);
    }catch(error){if(mounted.current){setTestJob(null);notify(t.testError+safeError(error instanceof Error?error.message:error))}}
  };

  if(view==='loading')return <div className="splash"><span className="loader"/></div>;
  if(view!=='app')return <div className="auth"><div className="auth-art"><div className="brand"><ShieldCheck/> Rust Proxy</div><div><span className="eyebrow">SUB2SOCKS</span><h1>{t.quick}</h1><p>{t.welcome}</p></div></div><div className="auth-panel"><div className="auth-tools"><button onClick={()=>setDark(!dark)}>{dark?<Sun/>:<Moon/>}</button><button onClick={()=>setLang(lang==='en'?'zh-CN':'en')}><Globe2/>{lang==='en'?'中':'EN'}</button></div><form className="auth-card" onSubmit={event=>auth(event,view==='setup'?'/api/init':'/api/auth/login')}><div className="auth-icon"><ShieldCheck/></div><h2>{view==='setup'?t.setup:t.login}</h2><p>{view==='setup'?t.setupDesc:t.loginDesc}</p><label>{t.admin}<input name="admin_user" required autoComplete="username"/></label><label>{t.password}<input name="admin_pass" type="password" required autoComplete={view==='setup'?'new-password':'current-password'} aria-describedby={authError?'auth-error':undefined}/></label>{authError&&<p id="auth-error" className="auth-error" role="alert">{authError}</p>}<button className="primary" disabled={busy}>{view==='setup'?t.setup:t.signIn}<ChevronRight/></button></form></div>{toast&&<div className="toast">{toast}</div>}</div>;

  const mutate=async(path:string,options:RequestInit,message:string)=>action(async()=>{await api(path,options);await load();notify(message)});
  return <div className="shell"><aside className={mobile?'open':''}><div className="brand"><ShieldCheck/> <span>Rust Proxy</span><button className="close" onClick={()=>setMobile(false)}><X/></button></div><nav>{nav.map(([id,Icon,label])=><button className={active===id?'active':''} onClick={()=>{setActive(id);setMobile(false)}} key={id}><Icon/>{label}</button>)}</nav><div className="aside-foot"><div className={'status-dot '+(mihomo.running?'on':'')}/><div><strong>{t.status}</strong><span>{mihomo.running?t.running:t.stopped}</span></div></div></aside>{mobile&&<div className="scrim" onClick={()=>setMobile(false)}/>}<main><header><button className="hamb" onClick={()=>setMobile(true)}><Menu/></button><div><span className="eyebrow">CONTROL CENTER</span><h1>{nav.find(item=>item[0]===active)?.[2]}</h1></div><div className="header-actions"><button onClick={()=>setLang(lang==='en'?'zh-CN':'en')}><Globe2/><span>{lang==='en'?'中':'EN'}</span></button><button onClick={()=>setDark(!dark)}>{dark?<Sun/>:<Moon/>}</button><button onClick={()=>action(load)}><RefreshCw/></button><button onClick={()=>action(async()=>{try{await api('/api/auth/logout',body({}))}finally{setView('login')}})}><LogOut/></button></div></header><div className="content">
    {active==='overview'&&<><div className="hero"><div><span className="eyebrow">MIHOMO CORE</span><h2>{t.welcome}</h2><p>{t.quick}</p></div><div className="hero-actions"><button className="primary" onClick={()=>mutate('/api/mihomo/start',body({}),'Mihomo started')}><Play/>{t.start}</button><button onClick={()=>mutate('/api/mihomo/stop',body({}),'Mihomo stopped')}><Square/>{t.stop}</button></div></div><div className="stats"><Stat icon={<Activity/>} label={t.status} value={mihomo.running?t.running:t.stopped}/><Stat icon={<Server/>} label={t.port} value={String(port)}/><Stat icon={<BookOpen/>} label={t.subs} value={String(subs.length)}/><Stat icon={<Network/>} label={t.nodes} value={String(nodes.length)}/></div></>}
    {active==='subscriptions'&&<Section title={t.subs} count={subs.length}><form className="form-row" onSubmit={async event=>{event.preventDefault();await mutate('/api/subscriptions',body(Object.fromEntries(new FormData(event.currentTarget))),t.addSub);event.currentTarget.reset()}}><label>{t.url}<input name="url" type="url" required placeholder="https://example.com/sub"/></label><label>{t.name}<input name="name" placeholder={t.optional}/></label><button className="primary">{t.addSub}</button></form><Table heads={[t.name,t.url,t.lastSync,'']} rows={subs.map(subscription=>[subscription.name,subscription.url,subscription.last_synced_at?new Date(Number(subscription.last_synced_at)*1000).toLocaleString():'-',<Actions sync={()=>mutate(`/api/subscriptions/${subscription.id}/sync`,body({}),t.sync)} del={()=>mutate(`/api/subscriptions/${subscription.id}`,{method:'DELETE'},t.del)} t={t}/>])} empty={t.empty}/></Section>}
    {active==='nodes'&&<Section title={t.nodes} count={nodes.length} action={<button className="primary" disabled={!!testJob||testingNodes.size>0} onClick={()=>void testAll()}><Activity/>{testJob?t.testing:t.testAll}</button>}>
      {testJob&&<div className="test-progress" role="status"><strong>{t.testProgress}: {testJob.done}/{testJob.total}</strong><span>{t.testSucceeded}: {testJob.ok}</span><span>{t.testFailed}: {testJob.failed}</span><progress max={Math.max(testJob.total,1)} value={testJob.done}/></div>}
      <Table heads={[t.enabled,t.name,t.type,t.server,t.subscription,t.lastTest,'']} rows={nodes.map(node=>{const subscriptionId=Number(node.subscription_id);const subscription=Number.isFinite(subscriptionId)&&subscriptionId>0?subs.find(item=>item.id===subscriptionId):undefined;const subscriptionLabel=subscription?.name||(node.subscription_id==null||node.subscription_id===''||!Number.isFinite(subscriptionId)||subscriptionId<=0?t.noSubscription:t.unknownSubscription);const testing=!!testJob||testingNodes.has(node.id);return [<input type="checkbox" checked={!!node.enabled} onChange={event=>mutate(`/api/nodes/${node.id}/enabled`,{method:'PUT',body:JSON.stringify({enabled:event.target.checked})},t.enabled)}/>,node.name,node.node_type,`${node.server||''}:${node.port||''}`,subscriptionLabel,<NodeTestStatus node={node} testing={testing} t={t}/>,<div className="actions"><button disabled={testing} onClick={()=>void testNode(node.id)}>{testing?t.testing:t.test}</button><button className="danger" onClick={()=>mutate(`/api/nodes/${node.id}`,{method:'DELETE'},t.del)}>{t.del}</button></div>]})} empty={t.empty}/>
    </Section>}
    {active==='accounts'&&<Section title={t.accounts} count={accounts.length}><AccountForm t={t} nodes={nodes} editing={editing} cancel={()=>setEditing(null)} save={async(value:any,id:string)=>{await mutate(id?`/api/socks-accounts/${id}`:'/api/socks-accounts',{method:id?'PUT':'POST',body:JSON.stringify(value)},t.save);setEditing(null)}}/><Table heads={[t.enabled,t.name,t.user,t.node,'']} rows={accounts.map(account=>[<input type="checkbox" checked={!!account.enabled} onChange={event=>mutate(`/api/socks-accounts/${account.id}/enabled`,{method:'PUT',body:JSON.stringify({enabled:event.target.checked})},t.enabled)}/>,account.name,account.username,nodes.find(node=>node.id===account.node_id)?.name||account.node_id,<div className="actions"><button onClick={()=>setEditing(account)}>{t.edit}</button><button className="danger" onClick={()=>mutate(`/api/socks-accounts/${account.id}`,{method:'DELETE'},t.del)}>{t.del}</button></div>])} empty={t.empty}/></Section>}
    {active==='config'&&<Section title={t.config}><button className="primary" onClick={()=>action(async()=>setYaml((await api('/api/config/preview')).yaml))}>{t.preview}</button><pre>{yaml||'# '+t.preview}</pre></Section>}
  </div></main>{busy&&<div className="loading"><span className="loader"/></div>}{toast&&<div className="toast">{toast}</div>}</div>;
}

function NodeTestStatus({node,testing,t}:{node:Item;testing:boolean;t:any}){
  if(testing)return <div className="test-result"><span className="status-badge testing">{t.testing}</span></div>;
  const testedAt=typeof node.last_tested_at==='number'&&Number.isFinite(node.last_tested_at)?node.last_tested_at:null;
  if(testedAt===null||typeof node.last_test_ok!=='boolean')return <div className="test-result"><span className="status-badge untested">{t.untested}</span></div>;
  const time=new Date(testedAt*1000);const formatted=Number.isNaN(time.getTime())?'-':time.toLocaleString();
  if(node.last_test_ok){const latency=typeof node.last_test_latency_ms==='number'&&Number.isFinite(node.last_test_latency_ms)?`${node.last_test_latency_ms} ms`:'-';return <div className="test-result"><span className="status-badge available">{t.available}</span><span className="test-detail">{t.latency}: {latency}</span><time dateTime={Number.isNaN(time.getTime())?undefined:time.toISOString()}>{formatted}</time></div>}
  const error=safeError(node.last_test_error)||'-';
  return <div className="test-result"><span className="status-badge unavailable">{t.unavailable}</span><span className="test-error" title={error}>{error}</span><time dateTime={Number.isNaN(time.getTime())?undefined:time.toISOString()}>{formatted}</time></div>;
}
function Stat(props:{icon:ReactNode;label:string;value:ReactNode}){return <div className="stat"><i>{props.icon}</i><div><span>{props.label}</span><strong>{props.value}</strong></div></div>}
function Section(props:{title:string;count?:number;action?:ReactNode;children:ReactNode}){return <section className="card"><div className="section-title"><div><h2>{props.title}</h2>{props.count!==undefined&&<span>{props.count}</span>}</div>{props.action&&<div className="section-action">{props.action}</div>}</div>{props.children}</section>}
function Table({heads,rows,empty}:{heads:string[];rows:ReactNode[][];empty:string}){return <div className="table-wrap">{rows.length?<table><thead><tr>{heads.map((head,index)=><th key={index}>{head}</th>)}</tr></thead><tbody>{rows.map((row,index)=><tr key={index}>{row.map((value,column)=><td key={column}>{value}</td>)}</tr>)}</tbody></table>:<div className="empty">{empty}</div>}</div>}
function Actions({sync,del,t}:{sync:()=>void;del:()=>void;t:any}){return <div className="actions"><button onClick={sync}>{t.sync}</button><button className="danger" onClick={del}>{t.del}</button></div>}
function AccountForm({t,nodes,save,editing,cancel}:any){return <form key={editing?.id||'new'} className="account-grid" onSubmit={async event=>{event.preventDefault();const fields:any=Object.fromEntries(new FormData(event.currentTarget));fields.node_id=Number(fields.node_id);await save(fields,editing?.id?String(editing.id):'');event.currentTarget.reset()}}><label>{t.name}<input name="name" defaultValue={editing?.name||''} required/></label><label>{t.user}<input name="username" defaultValue={editing?.username||''} required/></label><label>{t.password}<input name="password" type="password" defaultValue={editing?.password||''} required/></label><label>{t.node}<select name="node_id" defaultValue={editing?.node_id} required>{nodes.filter((node:any)=>node.enabled||node.id===editing?.node_id).map((node:any)=><option value={node.id} key={node.id}>{node.name} ({node.node_type})</option>)}</select></label><button className="primary">{t.save}</button>{editing&&<button type="button" onClick={cancel}>{t.cancel}</button>}</form>}
