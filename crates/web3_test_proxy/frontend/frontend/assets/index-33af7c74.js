var he=Object.defineProperty;var ue=(t,s,n)=>s in t?he(t,s,{enumerable:!0,configurable:!0,writable:!0,value:n}):t[s]=n;var B=(t,s,n)=>(ue(t,typeof s!="symbol"?s+"":s,n),n);import{r as o,j as e,R as b}from"./react-077fff36.js";import{c as me}from"./react-dom-a94e3221.js";import{L as E,B as xe}from"./react-router-dom-aacee83d.js";import{D as A,I as je}from"./luxon-4a3d1a6a.js";import{d as fe,e as se,f as S}from"./react-router-cec35629.js";import{J as Q}from"./react-json-tree-59dcb848.js";import{T as X,a as Y,b as F,c as M}from"./react-tabs-866748e4.js";import{c as oe}from"./react-confirm-box-2b8c3243.js";import"./color-8be6040b.js";import"./color-string-327061cf.js";import"./color-name-b7949e8c.js";import"./simple-swizzle-d8bc59f1.js";import"./is-arrayish-4445f582.js";import"./scheduler-765c72db.js";import"./@remix-run-671dfa1a.js";import"./@babel-90fae8f7.js";import"./react-base16-styling-9e0156d9.js";import"./base16-823cae66.js";import"./lodash.curry-9cd0b063.js";import"./clsx-1229b3e0.js";(function(){const s=document.createElement("link").relList;if(s&&s.supports&&s.supports("modulepreload"))return;for(const r of document.querySelectorAll('link[rel="modulepreload"]'))c(r);new MutationObserver(r=>{for(const i of r)if(i.type==="childList")for(const l of i.addedNodes)l.tagName==="LINK"&&l.rel==="modulepreload"&&c(l)}).observe(document,{childList:!0,subtree:!0});function n(r){const i={};return r.integrity&&(i.integrity=r.integrity),r.referrerPolicy&&(i.referrerPolicy=r.referrerPolicy),r.crossOrigin==="use-credentials"?i.credentials="include":r.crossOrigin==="anonymous"?i.credentials="omit":i.credentials="same-origin",i}function c(r){if(r.ep)return;r.ep=!0;const i=n(r);fetch(r.href,i)}})();const k=o.createContext({backendSettings:{backendUrl:"",bearerToken:"",enableBearerToken:!1},setBackendSettings:t=>{console.error(`setBackendSettings not implemented: ${t}`)},resetSettings:()=>{console.error("resetSettings not implemented")}}),pe=t=>{const s=window.localStorage.getItem("backendUrl")??"/api",n=window.localStorage.getItem("bearerToken")??"",c=window.localStorage.getItem("bearerTokenEnabled")==="1",r={backendUrl:s,bearerToken:n,enableBearerToken:c},[i,l]=o.useState(r),h=o.useCallback(u=>{window.localStorage.setItem("backendUrl",u.backendUrl),window.localStorage.setItem("bearerToken",u.bearerToken),window.localStorage.setItem("bearerTokenEnabled",u.enableBearerToken?"1":"0"),l(u)},[l]),d=o.useCallback(()=>{h({backendUrl:"/api",bearerToken:"",enableBearerToken:!1})},[h]);return e.jsx(k.Provider,{value:{backendSettings:i,setBackendSettings:h,resetSettings:d},children:t.children})};function C(t,s,n){const c=(n==null?void 0:n.headers)??new Headers,r=(n==null?void 0:n.method)??"GET",i=n==null?void 0:n.body;let l=s;if(s.startsWith("/"))t.backendUrl.endsWith("/")?l=t.backendUrl+s.substring(1):l=t.backendUrl+s;else throw new Error("Uri must start with /");return t.enableBearerToken&&c.append("Authorization","Bearer "+t.bearerToken),i&&c.append("Content-Type","application/json"),console.log("Calling backend: "+l),fetch(l,{method:r,headers:c,body:i})}class w{constructor(s,n,c){B(this,"config");B(this,"progress");B(this,"error");this.config=s,this.progress=n,this.error=c}}const re=o.createContext(new w(null,"",null)),ae=()=>o.useContext(re);function be(){const t=ae();if(t.config==null)throw new Error("Config not available");return t.config}const ge=t=>{const[s,n]=o.useState(new w(null,"",null)),{backendSettings:c}=o.useContext(k);return o.useEffect(()=>{(async()=>{const r=c.backendUrl+"/config";n(new w(null,`Connecting to ${r}`,null));let i=null;try{const l=await C(c,"/config");if(l.type==="opaque"){n(new w(null,"",`Failed to connect to ${r} due to CORS policy`));return}i=await l.text();const h=JSON.parse(i);if(!h.config){n(new w(null,"",`No config field found on endpoint ${r} `));return}n(new w(h.config,"",null))}catch(l){console.log("Error fetching config",l),i&&console.log("Response body: ",i),n(new w(null,"",`Failed to connect to ${r}`))}})()},[n,c]),e.jsx(re.Provider,{value:s,children:t.children})};const Z=()=>{const{backendSettings:t,setBackendSettings:s,resetSettings:n}=o.useContext(k),[c,r]=b.useState(t.backendUrl),i=p=>{r(p.target.value)},[l,h]=b.useState(t.bearerToken),[d,u]=b.useState(t.enableBearerToken),m=p=>{u(p.target.checked)},f=p=>{h(p.target.value)},x=()=>{s({backendUrl:c,bearerToken:l,enableBearerToken:d})},j=()=>{r(t.backendUrl),h(t.bearerToken),u(t.enableBearerToken)};o.useEffect(()=>{r(t.backendUrl),h(t.bearerToken),u(t.enableBearerToken)},[t]);const g=()=>{n()},N=()=>c!==t.backendUrl||l!==t.bearerToken||d!==t.enableBearerToken;return e.jsxs("div",{className:"backend-settings",children:[e.jsx("div",{children:"Backend settings"}),e.jsx("hr",{}),e.jsx("h3",{children:"Backend URL:"}),e.jsx("input",{type:"text",value:c,onChange:i}),e.jsx("hr",{}),e.jsx("h3",{children:"Backend security:"}),e.jsxs("p",{children:[e.jsx("span",{style:{fontWeight:"bold"},children:"Bearer authentication"})," - token is added to bearer header value."]}),e.jsx("div",{children:e.jsxs("label",{children:[e.jsx("input",{type:"checkbox",checked:d,onChange:m}),"Enabled"]})}),e.jsx("input",{type:"text",value:l,onChange:f,disabled:!d}),e.jsx("hr",{}),e.jsxs("div",{className:"box-line",children:[e.jsx("input",{type:"button",value:"Save",onClick:x,disabled:!N()}),e.jsx("input",{type:"button",value:"Cancel",onClick:j,disabled:!N()}),e.jsx("input",{type:"button",value:"Reset to default",onClick:g})]})]})};const ie=t=>{const s=A.now(),n=t.date,c=t.title,r="N/A",i="-";let l="Date not yet available";function h(d,u,m,f){return t.minimal?e.jsx("div",{title:f,className:"date-container",children:e.jsx("div",{className:"date-container-date",children:u})}):e.jsxs("div",{title:f,className:"date-container",children:[e.jsx("div",{className:"date-container-title",children:d}),e.jsx("div",{className:"date-container-date",children:u}),e.jsx("div",{className:"date-container-msg",children:m})]})}if(n==null)return h(c,r,i,l);try{const d=A.fromISO(n),u=A.now(),m=je.fromDateTimes(d,s);let f;m.length("days")>3?f=Math.floor(m.length("days"))+" days ago":m.length("hours")>3?f=Math.floor(m.length("hours"))+" hours ago":m.length("minutes")>3?f=Math.floor(m.length("minutes"))+" min. ago":f=Math.floor(m.length("seconds"))+" sec. ago";let x=d.toFormat("yyyy-LL-dd HH:mm:ss");return d.toFormat("yyyy-LL-dd")===u.toFormat("yyyy-LL-dd")&&(x=d.toFormat("HH:mm:ss")),l="Iso date: "+d.toUTC().toFormat("yyyy-LL-dd HH:mm:ss"),h(c,x,f,l)}catch(d){return h(c,"error",`${d}`,`${d}`)}},Ce=t=>{var n;const s=t.latestCall;return e.jsxs("div",{className:"call-box",children:[e.jsx("div",{className:"call-box-header",children:e.jsx("a",{href:`/frontend/call/${t.endpointKey}/${s.id}`,children:s.id})}),e.jsx(ie,{date:s.date,title:"Time",minimal:!0}),e.jsx("div",{className:"call-box-body",children:s.parsedRequest.length>0?e.jsxs(e.Fragment,{children:[e.jsx("div",{children:s.parsedRequest[0].method??"unknown"}),e.jsxs(e.Fragment,{children:[((n=s.parsedRequest[0].parsedCall)==null?void 0:n.to)&&e.jsxs("div",{children:[e.jsx("div",{children:"Contract :"}),e.jsx("div",{children:s.parsedRequest[0].parsedCall.to})]}),s.parsedRequest[0].parsedCall&&e.jsxs("div",{children:[e.jsx("div",{children:"ERC20 balance:"}),e.jsx("div",{children:s.parsedRequest[0].parsedCall.address})]})]})]}):e.jsx("div",{children:"unknown"})})]})},ke=t=>{var l;const[s,n]=b.useState(null),{backendSettings:c}=o.useContext(k),r=o.useCallback(async()=>{try{const d=await(await C(c,`/calls/${t.apikey}/${t.showAtOnce}`)).json();n(d)}catch(h){console.log(h),n(null)}},[t.refreshToken]);function i(h){return e.jsx(Ce,{endpointKey:t.apikey,latestCall:h},h.id)}return o.useEffect(()=>{r().then()},[r]),s===null?e.jsxs("div",{className:"latest-calls-box",children:[e.jsx("div",{children:e.jsx("h3",{children:t.apikey})}),"Loading...",e.jsx("hr",{})]}):s.error?e.jsxs("div",{className:"latest-calls-box",children:[e.jsx("div",{children:e.jsx("h3",{children:t.apikey})}),s.error,e.jsx("hr",{})]}):e.jsxs("div",{className:"latest-calls-box",children:[e.jsx("div",{children:e.jsx("h3",{children:t.apikey})}),(l=s==null?void 0:s.calls)==null?void 0:l.map(i),e.jsx("hr",{})]})},ve=()=>{const[t,s]=o.useState(0),{backendSettings:n}=o.useContext(k),[c,r]=o.useState([]),[i,l]=o.useState([]),[h,d]=o.useState(""),[u,m]=o.useState("10"),f=o.useCallback(async()=>{try{let j=[];h.length>0&&(j=h.split(",").map(v=>v.trim()),j=j.filter(v=>v.length>0));let p=(await(await C(n,"/keys/active")).json()).keys.splice(0).sort();l(p),j.length>0&&(p=j.filter(v=>p.indexOf(v)>=0)),r(p)}catch(j){console.log(j),r([])}},[h,r]);b.useEffect(()=>{console.log("Refreshing dashboard..."),f().then(()=>{setTimeout(()=>{s(t+1)},2e3)})},[s,t]);function x(j){return e.jsx(ke,{apikey:j,refreshToken:t,showAtOnce:parseInt(u)||10},j)}return e.jsxs("div",{children:[e.jsxs("div",{className:"monitor-filter",children:[e.jsxs("div",{className:"monitor-filter-row1",children:[e.jsx("div",{className:"monitor-filter-el1",children:"Columns from server:"}),e.jsx("div",{children:i.join(",")})]}),e.jsxs("div",{className:"monitor-filter-row2",children:[e.jsx("div",{className:"monitor-filter-el1",children:"Selected columns:"}),e.jsx("input",{className:"monitor-filter-row2-el2",type:"text",value:h,onChange:j=>d(j.target.value)})]}),e.jsxs("div",{className:"monitor-filter-row3",children:[e.jsx("div",{className:"monitor-filter-el1",children:"Last displayed calls:"}),e.jsx("input",{className:"monitor-filter-row3-el2",type:"text",value:u,onChange:j=>m(j.target.value)})]})]}),e.jsx("div",{className:"monitor-appkey-lister",children:c.map(x)})]})};const ee={scheme:"default",author:"chris kempson (http://chriskempson.com)",base00:"#181818",base01:"#282828",base02:"#383838",base03:"#585858",base04:"#b8b8b8",base05:"#d8d8d8",base06:"#e8e8e8",base07:"#f8f8f8",base08:"#ab4642",base09:"#dc9656",base0A:"#f7ca88",base0B:"#a1b56c",base0C:"#86c1b9",base0D:"#7cafc2",base0E:"#ba8baf",base0F:"#a16946"},ne=()=>{const t=fe(),{backendSettings:s}=o.useContext(k),[n,c]=o.useState(null),r=t.callNo?parseInt(t.callNo):-1,[i,l]=o.useState(r),h=o.useCallback(async()=>{try{const j=await(await C(s,`/call/${t.key}/${i}`)).json();c(j.call)}catch(x){console.log(x),c(null)}},[t,i]);o.useEffect(()=>{h().then()},[h]);let d=null;try{d=n!=null&&n.request?JSON.parse(n==null?void 0:n.request):null}catch(x){console.log(x)}let u=null;try{u=n!=null&&n.response?JSON.parse(n==null?void 0:n.response):null}catch(x){console.log(x)}if(i<0)return e.jsx("div",{className:"single-call-info",children:e.jsx("h3",{children:"Call no is not specified"})});const m=(x,j,g)=>!0;function f(x){l(x),window.history.replaceState(null,"",`/frontend/call/${t.key}/${x}`)}return e.jsxs("div",{className:"single-call-info",children:[e.jsx("button",{style:{margin:"0 0.5rem 0.5rem 0"},disabled:i==0,onClick:()=>f(i-1),children:"Previous call"}),e.jsx("button",{onClick:()=>f(i+1),children:"Next call"}),e.jsx("table",{children:e.jsxs("tbody",{children:[e.jsxs("tr",{children:[e.jsx("th",{children:"Web3 endpoint key"}),e.jsx("td",{children:t.key})]}),e.jsxs("tr",{children:[e.jsx("th",{children:"Call no"}),e.jsx("td",{children:n==null?void 0:n.id})]}),e.jsxs("tr",{children:[e.jsx("th",{children:"Call time"}),e.jsx("td",{children:e.jsx(ie,{date:n==null?void 0:n.date,title:"Call time",minimal:!1})})]}),e.jsxs("tr",{children:[e.jsx("th",{children:"Response time"}),e.jsx("td",{children:n==null?void 0:n.responseTime})]})]})}),e.jsxs("div",{children:[e.jsx("h3",{children:"Request"}),e.jsxs(X,{children:[e.jsxs(Y,{children:[e.jsx(F,{children:"JSON"}),e.jsx(F,{children:"Raw"})]}),e.jsx(M,{children:d&&e.jsx(Q,{shouldExpandNodeInitially:m,theme:ee,invertTheme:!0,data:d})}),e.jsx(M,{children:e.jsx("code",{children:n==null?void 0:n.request})})]})]}),e.jsxs("div",{children:[e.jsx("h3",{children:"Response"}),e.jsxs(X,{children:[e.jsxs(Y,{children:[e.jsx(F,{children:"JSON"}),e.jsx(F,{children:"Raw"})]}),e.jsx(M,{selected:!1,children:u&&e.jsx(Q,{shouldExpandNodeInitially:m,theme:ee,invertTheme:!0,data:u})}),e.jsx(M,{selected:!0,children:e.jsx("code",{children:n==null?void 0:n.response})})]})]})]})};const ye=t=>{b.useEffect(()=>{console.log("Refreshing dashboard...")},[t]);const[s,n]=o.useState(null),{backendSettings:c}=o.useContext(k),[r,i]=o.useState(""),[l,h]=o.useState(""),[d,u]=o.useState(""),[m,f]=o.useState(""),[x,j]=o.useState(""),[g,N]=o.useState(""),[p,v]=o.useState(""),[D,O]=o.useState(""),[H,$]=o.useState(""),[V,U]=o.useState(""),[_,P]=o.useState(""),[J,I]=o.useState(""),[W,L]=o.useState(""),[K,q]=o.useState(""),[T,R]=o.useState(0),z=o.useCallback(async()=>{try{const y=await(await C(c,`/problems/${t.apikey}`)).json();n(y.problems),i(y.problems.errorChance.toString()),h(y.problems.timeoutChance.toString()),u(y.problems.minTimeoutMs.toString()),f(y.problems.maxTimeoutMs.toString()),j(y.problems.malformedResponseChance.toString()),N(y.problems.skipSendingRawTransactionChance.toString()),v(y.problems.sendTransactionButReportFailureChance.toString())}catch(a){console.log(a),n(null)}},[n,T]);b.useEffect(()=>{z().then(()=>{})},[z]),b.useEffect(()=>{const a=parseFloat(r);isNaN(a)?O("Not a number"):a>=0&&a<=1?O(""):O("Has to be number between 0.0 and 1.0")},[r]),b.useEffect(()=>{const a=parseFloat(l);isNaN(a)?$("Not a number"):a>=0&&a<=1?$(""):$("Has to be number between 0.0 and 1.0")},[l]),b.useEffect(()=>{const a=parseFloat(d);isNaN(a)?U("Not a number"):a>=0&&a<=1e5?U(""):U("Has to be number >= 0")},[d]),b.useEffect(()=>{const a=parseFloat(m);isNaN(a)?P("Not a number"):a>=0&&a<=1e5?P(""):P("Has to be number >= 0")},[m]),b.useEffect(()=>{const a=parseFloat(x);isNaN(a)?I("Not a number"):a>=0&&a<=1?I(""):I("Has to be number between 0.0 and 1.0")},[x]),b.useEffect(()=>{const a=parseFloat(g);isNaN(a)?L("Not a number"):a>=0&&a<=1?(console.log("Updating error chance to "+a),L("")):L("Has to be number between 0.0 and 1.0")},[g]),b.useEffect(()=>{const a=parseFloat(p);isNaN(a)?q("Not a number"):a>=0&&a<=1?(console.log("Updating error chance to "+a),q("")):q("Has to be number between 0.0 and 1.0")},[p]);const ce=o.useCallback(async()=>{if(s){console.log("Saving problems");const a={errorChance:parseFloat(r),timeoutChance:parseFloat(l),minTimeoutMs:parseFloat(d),maxTimeoutMs:parseFloat(m),malformedResponseChance:parseFloat(x),skipSendingRawTransactionChance:parseFloat(g),sendTransactionButReportFailureChance:parseFloat(p),allowOnlyParsedCalls:s.allowOnlyParsedCalls,allowOnlySingleCalls:s.allowOnlySingleCalls};await C(c,`/problems/set/${t.apikey}`,{method:"POST",body:JSON.stringify(a)}),R(T+1)}},[s,T,R,r,l,d,m,x,g,p]),de=o.useCallback(async()=>{await oe("Are you sure you want to delete all endpoint history?")&&(await C(c,`/keys/delete/${t.apikey}`,{method:"POST"}),R(T+1))},[T,R,t]);if(s===null)return e.jsx("div",{className:"endpoint",children:e.jsx("div",{children:"loading..."})});let G=D!==""||H!==""||V!==""||_!==""||J!==""||W!==""||K!=="";return r===s.errorChance.toString()&&l===s.timeoutChance.toString()&&d===s.minTimeoutMs.toString()&&m===s.maxTimeoutMs.toString()&&x===s.malformedResponseChance.toString()&&g===s.skipSendingRawTransactionChance.toString()&&p===s.sendTransactionButReportFailureChance.toString()&&(G=!0),e.jsxs("div",{className:"endpoint",children:[e.jsxs("div",{className:"endpoint-header-title",children:["Endpoint ",t.apikey]}),e.jsx("div",{children:JSON.stringify(s)}),e.jsx("table",{children:e.jsxs("tbody",{children:[e.jsxs("tr",{children:[e.jsx("th",{children:"Error chance per request"}),e.jsx("td",{children:e.jsx("input",{value:r,onChange:a=>i(a.target.value)})}),e.jsx("td",{children:s.errorChance}),e.jsx("td",{children:e.jsx("div",{children:D})})]}),e.jsxs("tr",{children:[e.jsx("th",{children:"Timeout chance per request"}),e.jsx("td",{children:e.jsx("input",{value:l,onChange:a=>h(a.target.value)})}),e.jsx("td",{children:s.timeoutChance}),e.jsx("td",{children:e.jsx("div",{children:H})})]}),e.jsxs("tr",{children:[e.jsx("th",{children:"Minimal timeout in ms"}),e.jsx("td",{children:e.jsx("input",{value:d,onChange:a=>u(a.target.value)})}),e.jsx("td",{children:s.minTimeoutMs}),e.jsx("td",{children:e.jsx("div",{children:V})})]}),e.jsxs("tr",{children:[e.jsx("th",{children:"Maximum timeout in ms"}),e.jsx("td",{children:e.jsx("input",{value:m,onChange:a=>f(a.target.value)})}),e.jsx("td",{children:s.maxTimeoutMs}),e.jsx("td",{children:e.jsx("div",{children:_})})]}),e.jsxs("tr",{children:[e.jsx("th",{children:"Malformed response chance"}),e.jsx("td",{children:e.jsx("input",{value:x,onChange:a=>j(a.target.value)})}),e.jsx("td",{children:s.malformedResponseChance}),e.jsx("td",{children:e.jsx("div",{children:J})})]}),e.jsxs("tr",{children:[e.jsx("th",{children:"Skip sending chance"}),e.jsx("td",{children:e.jsx("input",{value:g,onChange:a=>N(a.target.value)})}),e.jsx("td",{children:s.skipSendingRawTransactionChance}),e.jsx("td",{children:e.jsx("div",{children:W})})]}),e.jsxs("tr",{children:[e.jsx("th",{children:"Send but report error chance"}),e.jsx("td",{children:e.jsx("input",{value:p,onChange:a=>v(a.target.value)})}),e.jsx("td",{children:s.sendTransactionButReportFailureChance}),e.jsx("td",{children:e.jsx("div",{children:K})})]})]})}),e.jsx("button",{onClick:()=>de(),children:"Delete"}),e.jsx("button",{disabled:G,onClick:()=>ce(),children:"Save"})]})},Se=()=>{const[t,s]=o.useState([]),[n,c]=o.useState(0),{backendSettings:r}=o.useContext(k),i=o.useCallback(async()=>{try{const m=await(await C(r,"/keys")).json();s(m.keys)}catch(u){console.log(u),s([])}},[s]);b.useEffect(()=>{console.log("Refreshing dashboard..."),i().then(()=>{})},[i,n]);const l=o.useCallback(async()=>{await oe("Are you sure you want to delete all endpoints history?")&&(await C(r,"/keys/delete_all",{method:"POST"}),c(n+1))},[n,c]),h=o.useCallback(async()=>{c(n+1),s([])},[n,c]);function d(u){return e.jsx(ye,{apikey:u},u)}return e.jsxs("div",{className:"endpoints",children:[e.jsxs("div",{className:"endpoints-header",children:[e.jsx("button",{onClick:()=>h(),children:"Refresh"}),e.jsx("button",{onClick:()=>l(),children:"Delete All Endpoints"})]}),t.map(d)]})};const we=()=>{const{backendSettings:t}=o.useContext(k),s=be();return e.jsxs("div",{className:"welcome-page",children:[e.jsx("h1",{children:"Web3 proxy"}),e.jsxs("p",{children:["Connected to the endpoint ",t.backendUrl]}),e.jsxs("p",{children:["Frontend version ","0.1.1"]}),e.jsxs("p",{children:["Backend version ",s.version]})]})},Ne=()=>{const t=ae();return t.error?e.jsxs("div",{children:[e.jsx("div",{children:t.error}),e.jsx(Z,{})]}):t.config==null?e.jsxs("div",{children:["Loading... ",t.progress]}):e.jsx("div",{children:e.jsxs("div",{children:[e.jsxs("div",{className:"top-header",children:[e.jsx("div",{className:"top-header-title",children:"Web3 proxy panel"}),e.jsxs("div",{className:"top-header-navigation",children:[e.jsx(E,{to:"/",children:"Main"}),e.jsx(E,{to:"/monitor",children:"Monitor"}),e.jsx(E,{to:"/endpoints",children:"Endpoints"}),e.jsx(E,{to:"/page3",children:"Page 3"})]})]}),e.jsx("div",{className:"main-content",children:e.jsxs(se,{children:[e.jsx(S,{path:"/",element:e.jsx(we,{})}),e.jsx(S,{path:"monitor",element:e.jsx("div",{children:e.jsx(ve,{})})}),e.jsx(S,{path:"/call/:key/:callNo",element:e.jsx(ne,{})}),e.jsx(S,{path:"call",element:e.jsx("div",{children:e.jsx(ne,{})})}),e.jsx(S,{path:"endpoints",element:e.jsx("div",{children:e.jsx(Se,{})})}),e.jsx(S,{path:"page3",element:e.jsx("div",{children:e.jsx(Z,{})})})]})})]})})},le=document.getElementById("root");if(!le)throw new Error("No root element found");const te=me.createRoot(le),Te=document.baseURI;Te.includes("/frontend/")?te.render(e.jsx(b.StrictMode,{children:e.jsx(pe,{children:e.jsx(ge,{children:e.jsx(xe,{basename:"/frontend/",children:e.jsx(se,{children:e.jsx(S,{path:"/*",element:e.jsx(Ne,{})})})})})})})):te.render(e.jsx("div",{children:e.jsxs("p",{children:["Invalid base URI, navigate to ","/frontend/"]})}));
