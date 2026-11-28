use super::MAX_PAGE_CONTEXT_CHARS;
use crate::cleaner::{decode_basic_entities, strip_tag_blocks};
use serde_json::Value;

#[derive(Debug, Clone)]
pub struct PageContext {
    pub title: String,
    pub url: String,
    pub text: String,
}

pub fn extract_page_context_from_html(raw_html: &str, title: &str, url: &str) -> PageContext {
    let stripped = strip_tag_blocks(
        raw_html,
        &["script", "style", "noscript", "svg", "nav", "footer", "aside", "iframe"],
    );
    let mut text = String::with_capacity(stripped.len().min(MAX_PAGE_CONTEXT_CHARS));
    let mut in_tag = false;
    let mut prev_space = true;
    let mut n_chars = 0usize;
    for ch in stripped.chars() {
        let pushed = match ch {
            '<' => {
                in_tag = true;
                let push = !prev_space;
                if push {
                    text.push(' ');
                    prev_space = true;
                }
                push
            }
            '>' => {
                in_tag = false;
                false
            }
            _ if in_tag => false,
            c if c.is_whitespace() => {
                let push = !prev_space;
                if push {
                    text.push(if c == '\n' { '\n' } else { ' ' });
                    prev_space = true;
                }
                push
            }
            c => {
                text.push(c);
                prev_space = false;
                true
            }
        };
        n_chars += usize::from(pushed);
        if n_chars >= MAX_PAGE_CONTEXT_CHARS + 256 {
            break;
        }
    }
    let decoded = decode_basic_entities(text.trim());
    let capped: String = decoded.chars().take(MAX_PAGE_CONTEXT_CHARS).collect();
    PageContext {
        title: title.to_string(),
        url: url.to_string(),
        text: capped,
    }
}

pub fn build_page_context_from_text(title: &str, url: &str, raw_text: &str) -> PageContext {
    let mut out = String::with_capacity(raw_text.len().min(MAX_PAGE_CONTEXT_CHARS));
    let mut prev_nl = 0usize;
    let mut n_chars = 0usize;
    for line in raw_text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            if prev_nl < 1 && !out.is_empty() {
                out.push('\n');
                n_chars += 1;
                prev_nl += 1;
            }
            continue;
        }
        prev_nl = 0;
        if !out.is_empty() {
            out.push('\n');
            n_chars += 1;
        }
        out.push_str(trimmed);
        n_chars += trimmed.chars().count();
        if n_chars >= MAX_PAGE_CONTEXT_CHARS {
            break;
        }
    }
    let capped: String = out.chars().take(MAX_PAGE_CONTEXT_CHARS).collect();
    PageContext {
        title: title.to_string(),
        url: url.to_string(),
        text: capped,
    }
}

pub(super) const UNTRUSTED_TAG: &str = "untrusted_webpage_content";
pub(super) const UNTRUSTED_STATE_TAG: &str = "untrusted_agent_state";
pub(super) const HISTORY_TAG: &str = "conversation_history";

pub(super) fn replace_ascii_tag_ci(s: &str, needle: &str, replacement: &str) -> String {
    let lower = s.to_ascii_lowercase(); // ASCII lowercasing keeps byte offsets identical.
    let mut out = String::with_capacity(s.len());
    let mut last = 0;
    for (i, _) in lower.match_indices(needle) {
        out.push_str(&s[last..i]);
        out.push_str(replacement);
        last = i + needle.len();
    }
    out.push_str(&s[last..]);
    out
}

/// Page text must not be able to close (or reopen) the `<untrusted_webpage_content>`,
/// `<untrusted_agent_state>`, or `<conversation_history>` wrappers and smuggle text out
/// as if the user wrote it, so every spelling of those tag names is defanged.
pub(super) fn neutralize_untrusted(s: &str) -> String {
    let s = replace_ascii_tag_ci(s, UNTRUSTED_TAG, "untrusted-webpage-content");
    let s = replace_ascii_tag_ci(&s, UNTRUSTED_STATE_TAG, "untrusted-agent-state");
    replace_ascii_tag_ci(&s, HISTORY_TAG, "conversation-history")
}

pub(super) fn sanitize_attr_meta(s: &str, max_chars: usize) -> String {
    neutralize_untrusted(s)
        .chars()
        .take(max_chars)
        .map(|c| match c {
            '"' => '\'',
            '\r' | '\n' | '<' | '>' => ' ',
            _ => c,
        })
        .collect()
}

pub(super) fn format_user_turn(user_text: &str, page: Option<&PageContext>) -> String {
    match page {
        Some(ctx) => {
            let title = sanitize_attr_meta(&ctx.title, 180);
            let url = sanitize_attr_meta(&ctx.url, 300);
            let body = if ctx.text.trim().is_empty() {
                "(빈 페이지 또는 추출된 텍스트 없음)".to_string()
            } else {
                neutralize_untrusted(&ctx.text)
            };
            format!(
                "<{UNTRUSTED_TAG} title=\"{title}\" url=\"{url}\">\n{body}\n</{UNTRUSTED_TAG}>\n\nUser request:\n{user_text}"
            )
        }
        None => user_text.to_string(),
    }
}

#[derive(Debug, Clone)]
pub struct AgentContext {
    pub step: usize,
    pub max_steps: usize,
    pub action_log: Vec<String>,
    pub notes: Vec<String>,
    /// PNG (base64) from the agent's `screenshot` tool, attached to this round's user turn.
    pub image_png_b64: Option<String>,
}

pub const AGENT_SNAPSHOT_JS: &str = r#"(function(){try{
  var sel='a[href],button,input,textarea,select,summary,label[for],[role="button"],[role="link"],[role="textbox"],[role="searchbox"],[role="checkbox"],[role="radio"],[role="switch"],[role="tab"],[role="option"],[role="combobox"],[role="menuitem"],[role="menuitemcheckbox"],[role="treeitem"],[contenteditable="true"],[tabindex]';
  window.__rbAid=Object.create(null);
  var vh=window.innerHeight||800;var vw=window.innerWidth||1200;
  var cands=[];var seen=typeof Set!=='undefined'?new Set():null;
  function hasAncestorOrChild(el){
    if(!seen)return false;
    var p=el.parentElement;
    while(p){if(seen.has(p))return true;p=p.parentElement;}
    return false;
  }
  function getLabel(el,root,doc){
    var al=el.getAttribute('aria-label');
    if(al&&al.trim())return al.replace(/\s+/g,' ').trim().slice(0,56);
    var alb=el.getAttribute('aria-labelledby');
    if(alb){
      var parts=[];
      alb.trim().split(/\s+/).forEach(function(lid){
        var lr=(root&&root.getElementById?root.getElementById(lid):null)||(doc&&doc.getElementById?doc.getElementById(lid):null);
        if(lr&&lr.innerText)parts.push(lr.innerText.trim());
      });
      if(parts.length)return parts.join(' ').replace(/\s+/g,' ').trim().slice(0,56);
    }
    if(el.id&&doc){
      try{
        var esc=(typeof CSS!=='undefined'&&CSS.escape)?CSS.escape(el.id):el.id.replace(/"/g,'\\"');
        var fl=doc.querySelector('label[for="'+esc+'"]');
        if(fl&&fl.innerText)return fl.innerText.replace(/\s+/g,' ').trim().slice(0,56);
      }catch(e){}
    }
    if(el.closest){var cl=el.closest('label');if(cl&&cl.innerText)return cl.innerText.replace(/\s+/g,' ').trim().slice(0,56);}
    var raw=el.getAttribute('placeholder')||el.innerText||el.value||el.title||el.getAttribute('name')||'';
    raw=String(raw).replace(/\s+/g,' ').trim();
    if(!raw){var im=el.querySelector&&el.querySelector('img[alt]');if(im&&im.alt)raw=im.alt.trim();}
    if(!raw){var st=el.querySelector&&el.querySelector('svg title');if(st&&st.textContent)raw=st.textContent.trim();}
    return raw.replace(/\s+/g,' ').trim().slice(0,56);
  }
  function pushCand(el,ox,oy,root,doc,explicit){
    if(cands.length>=140)return;
    if(el.disabled)return;
    if(seen&&seen.has(el))return;
    var r=el.getBoundingClientRect();
    if(r.width<4||r.height<4)return;
    if(!explicit&&(r.width>vw*0.85||r.height>vh*0.5))return;
    var top=oy+r.top,bot=oy+r.bottom,left=ox+r.left,right=ox+r.right;
    if(bot<-250||top>vh+500||right<-50||left>vw+250)return;
    var cs;try{cs=getComputedStyle(el);}catch(e){return;}
    if(cs.display==='none'||cs.visibility==='hidden'||parseFloat(cs.opacity)===0)return;
    var tag=el.tagName.toLowerCase();
    var kind=el.getAttribute('type')||el.getAttribute('role')||'';
    var label=getLabel(el,root,doc);
    if(!explicit&&!label)return;
    if(seen)seen.add(el);
    var inVp=bot>0&&top<vh&&right>0&&left<vw;
    var val='';var opts=[];var chk=null;
    if(tag==='select'){
      var so=el.options&&el.selectedIndex>=0?el.options[el.selectedIndex]:null;
      val=so?String(so.text||'').replace(/\s+/g,' ').trim().slice(0,48):'';
      if(el.options){for(var j=0;j<el.options.length&&j<15;j++){var ot=String(el.options[j].text||'').replace(/\s+/g,' ').trim();if(ot)opts.push(ot.slice(0,40));}}
    }else if(tag==='input'||tag==='textarea'){
      var kt=kind.toLowerCase();
      if(kt==='checkbox'||kt==='radio'){chk=Boolean(el.checked);}
      val=(kt==='password')?'[masked]':String(el.value||'').replace(/\s+/g,' ').trim().slice(0,48);
    }else if(el.isContentEditable){
      val=String(el.innerText||'').replace(/\s+/g,' ').trim().slice(0,48);
    }else if(kind==='checkbox'||kind==='switch'||kind==='menuitemcheckbox'){
      chk=el.getAttribute('aria-checked')==='true';
    }
    var exp=el.getAttribute('aria-expanded');
    var aSel=el.getAttribute('aria-selected');
    var rawHref=(tag==='a')?String(el.getAttribute('href')||'').trim():'';
    var href=(rawHref&&rawHref[0]!=='#'&&rawHref.indexOf('javascript:')!==0)?rawHref.split(/[?#]/)[0].slice(0,48):'';
    var focused=(doc&&doc.activeElement===el);
    cands.push({el:el,inVp:inVp,top:top,left:left,tag:tag,kind:kind,label:label,val:val,href:href,chk:chk,exp:exp,aSel:aSel,opts:opts,focused:focused});
  }
  function scan(root,doc,ox,oy,depth){
    if(!root||cands.length>=140||depth>4)return;
    var nodes;try{nodes=root.querySelectorAll(sel);}catch(e){return;}
    for(var i=0;i<nodes.length&&cands.length<140;i++){pushCand(nodes[i],ox,oy,root,doc,true);}
    var ptrs;try{ptrs=root.querySelectorAll('div,span,li,td');}catch(e){ptrs=[];}
    for(var pi=0;pi<ptrs.length&&pi<600&&cands.length<140;pi++){
      var pe=ptrs[pi];
      if(pe.offsetParent===null||pe.childElementCount>4)continue;
      if(hasAncestorOrChild(pe))continue;
      var pcs;try{pcs=getComputedStyle(pe);}catch(e){continue;}
      if(pcs.cursor==='pointer'){pushCand(pe,ox,oy,root,doc,false);}
    }
    var iframes;try{iframes=root.querySelectorAll('iframe');}catch(e){iframes=[];}
    for(var fi=0;fi<iframes.length&&cands.length<140;fi++){
      try{if(iframes[fi].contentDocument){var fr=iframes[fi].getBoundingClientRect();scan(iframes[fi].contentDocument,iframes[fi].contentDocument,ox+fr.left,oy+fr.top,depth+1);}}catch(e){}
    }
    var all;try{all=root.querySelectorAll('*');}catch(e){return;}
    for(var k=0;k<all.length&&k<2000&&cands.length<140;k++){
      if(all[k].shadowRoot)scan(all[k].shadowRoot,doc,ox,oy,depth+1);
    }
  }
  scan(document,document,0,0,0);
  cands.sort(function(a,b){
    if(a.inVp!==b.inVp)return a.inVp?-1:1;
    var dt=Math.round(a.top/16)-Math.round(b.top/16);
    if(dt!==0)return dt;
    return a.left-b.left;
  });
  var out=[];var focusedId=0;
  for(var idx=0;idx<cands.length&&idx<55;idx++){
    var c=cands[idx];var id=idx+1;
    window.__rbAid[id]=c.el;
    try{c.el.setAttribute('data-rb-aid',String(id));}catch(e){}
    if(c.focused)focusedId=id;
    var rec={id:id,tag:c.tag,kind:c.kind,label:c.label,val:c.val,href:c.href};
    if(c.chk!==null)rec.chk=c.chk;
    if(c.exp==='true'||c.exp==='false')rec.exp=(c.exp==='true');
    if(c.aSel==='true')rec.sel=true;
    if(c.opts.length)rec.opts=c.opts;
    out.push(rec);
  }
  var se=document.scrollingElement||document.documentElement||document.body;
  var sy=se?Math.round(se.scrollTop||window.scrollY||0):0;
  var ms=se?Math.max(0,Math.round((se.scrollHeight||0)-(se.clientHeight||vh))):0;
  if(ms<=20){
    var bestArea=0;
    var alls=document.querySelectorAll('main,div,section,aside');
    for(var si=0;si<alls.length&&si<400;si++){
      var ae=alls[si];
      if(ae.clientHeight>=150&&ae.scrollHeight>ae.clientHeight+20){
        var acs=getComputedStyle(ae);
        if(acs.overflowY==='auto'||acs.overflowY==='scroll'){
          var ar=ae.clientWidth*ae.clientHeight;
          if(ar>bestArea){bestArea=ar;sy=Math.round(ae.scrollTop);ms=Math.round(ae.scrollHeight-ae.clientHeight);}
        }
      }
    }
  }
  var hasDialog=Boolean(document.querySelector('[role="dialog"],dialog[open],.modal.show'));
  var bodyText=(document.body?document.body.innerText:'').slice(0,2500);
  return JSON.stringify({title:document.title||'',url:location.href||'',text:bodyText,elements:out,scrollY:sy,maxScroll:ms,focusedId:focusedId,hasDialog:hasDialog});
}catch(e){return JSON.stringify({title:document.title||'',url:location.href||'',text:'',elements:[],err:String(e)});}})()"#;

pub(super) fn cap_snapshot_str(s: &str, max_chars: usize) -> String {
    s.chars().take(max_chars).collect()
}

pub fn format_agent_snapshot(raw_json: &str, fallback_title: &str, fallback_url: &str) -> PageContext {
    let val: Value = serde_json::from_str(raw_json)
        .ok()
        .and_then(|outer| match outer {
            Value::String(inner) => serde_json::from_str(&inner).ok(),
            other => Some(other),
        })
        .unwrap_or(Value::Null);

    let title = val
        .get("title")
        .and_then(|v| v.as_str())
        .filter(|s| !s.trim().is_empty())
        .map(|s| cap_snapshot_str(s, 180))
        .unwrap_or_else(|| cap_snapshot_str(fallback_title, 180));
    let url = val
        .get("url")
        .and_then(|v| v.as_str())
        .filter(|s| !s.trim().is_empty())
        .map(|s| cap_snapshot_str(s, 400))
        .unwrap_or_else(|| cap_snapshot_str(fallback_url, 400));
    let raw_text = val.get("text").and_then(|v| v.as_str()).unwrap_or("");
    let cleaned = build_page_context_from_text(&title, &url, raw_text);

    let mut out = String::new();
    let mut state_parts = Vec::new();
    if let (Some(sy), Some(ms)) = (
        val.get("scrollY").and_then(|v| v.as_u64()),
        val.get("maxScroll").and_then(|v| v.as_u64()),
    ) && (sy > 0 || ms > 0)
    {
        if let Some(pct) = sy.saturating_mul(100).checked_div(ms) {
            state_parts.push(format!("Scroll: {}% ({sy}/{ms}px)", pct.min(100)));
        } else {
            state_parts.push("Scroll: Top (0px)".to_string());
        }
    }
    if let Some(fid) = val.get("focusedId").and_then(|v| v.as_u64())
        && fid > 0
    {
        state_parts.push(format!("Focused: [{fid}]"));
    }
    if val.get("hasDialog").and_then(|v| v.as_bool()) == Some(true) {
        state_parts.push("Modal: OPEN".to_string());
    }
    if !state_parts.is_empty() {
        out.push_str("[PAGE STATE]\n");
        out.push_str(&state_parts.join(" | "));
        out.push_str("\n\n");
    }

    out.push_str("[INTERACTIVE ELEMENTS]\n");
    if let Some(elems) = val.get("elements").and_then(|v| v.as_array()) {
        if elems.is_empty() {
            out.push_str("(보이는 상호작용 요소 없음 — scroll 또는 navigate 가능)\n");
        }
        for el in elems.iter().take(55) {
            let id = el.get("id").and_then(|v| v.as_u64()).unwrap_or(0);
            let tag = cap_snapshot_str(el.get("tag").and_then(|v| v.as_str()).unwrap_or("el"), 16);
            let kind = cap_snapshot_str(el.get("kind").and_then(|v| v.as_str()).unwrap_or(""), 24);
            let label = cap_snapshot_str(el.get("label").and_then(|v| v.as_str()).unwrap_or(""), 100);
            let val_s = cap_snapshot_str(el.get("val").and_then(|v| v.as_str()).unwrap_or(""), 64);
            let href = cap_snapshot_str(el.get("href").and_then(|v| v.as_str()).unwrap_or(""), 120);
            let chk = el.get("chk").and_then(|v| v.as_bool());
            let exp = el.get("exp").and_then(|v| v.as_bool());
            let sel_st = el.get("sel").and_then(|v| v.as_bool());
            let opts: Vec<String> = el
                .get("opts")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .take(8)
                        .filter_map(|o| o.as_str().map(|s| cap_snapshot_str(s, 40)))
                        .collect()
                })
                .unwrap_or_default();

            out.push('[');
            out.push_str(&id.to_string());
            out.push_str("] <");
            out.push_str(&tag);
            if !kind.is_empty() {
                out.push(' ');
                out.push_str(&kind);
            }
            out.push('>');
            if !label.is_empty() {
                out.push_str(" \"");
                out.push_str(&label);
                out.push('"');
            }
            if let Some(c) = chk {
                out.push_str(if c { " checked=true" } else { " checked=false" });
            }
            if let Some(e) = exp {
                out.push_str(if e { " expanded=true" } else { " expanded=false" });
            }
            if sel_st == Some(true) {
                out.push_str(" selected=true");
            }
            if !val_s.is_empty() {
                out.push_str(" val=\"");
                out.push_str(&val_s);
                out.push('"');
            }
            if !opts.is_empty() {
                out.push_str(" opts=[");
                out.push_str(&opts.join("|"));
                out.push(']');
            }
            if !href.is_empty() {
                out.push_str(" href=");
                out.push_str(&href);
            }
            out.push('\n');
        }
    } else {
        out.push_str("(요소 목록 없음)\n");
    }

    if !cleaned.text.is_empty() {
        out.push_str("\n[PAGE TEXT]\n");
        let body: String = cleaned.text.chars().take(1600).collect();
        out.push_str(&body);
    }

    PageContext { title, url, text: out }
}
