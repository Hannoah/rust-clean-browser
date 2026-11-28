use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentAction {
    Navigate {
        url: String,
    },
    Click {
        target: usize,
    },
    ClickText {
        text: String,
    },
    Type {
        target: usize,
        text: String,
        submit: bool,
        clear: bool,
    },
    Select {
        target: usize,
        value: String,
    },
    Hover {
        target: usize,
    },
    Press {
        key: String,
    },
    Scroll {
        direction: String,
        amount: i32,
        target: Option<usize>,
    },
    OpenTab {
        url: String,
    },
    SwitchTab {
        tab: usize,
    },
    CloseTab {
        tab: usize,
    },
    Extract {
        kind: String,
        copy: bool,
    },
    Back,
    Forward,
    Reload,
    Wait {
        ms: u64,
    },
    WaitFor {
        text: String,
        timeout_ms: u64,
    },
    Done {
        summary: String,
    },
    // ── Data tools: their result text is handed to the next round ([EXTRACTED DATA]) ──
    /// Find text on the page, scroll to the first hit, return count + snippets.
    FindInPage {
        text: String,
    },
    /// Read another tab's text without switching to it.
    ReadTab {
        tab: usize,
    },
    /// Show a page of the open PDF and return its text.
    PdfPage {
        page: u32,
    },
    /// Search the whole open PDF; jump to the first hit.
    PdfSearch {
        text: String,
    },
    /// Fill several fields at once (never submits).
    FillForm {
        fields: Vec<(usize, String)>,
    },
    BookmarksSearch {
        query: String,
    },
    HistorySearch {
        query: String,
    },
    /// Citation for the current page: `apa` | `ieee` | `mla` | `slide`.
    Cite {
        style: String,
    },
    /// Wait until the page stops loading resources.
    WaitForNetworkIdle {
        timeout_ms: u64,
    },
    /// Capture the visible page as an image for the next round (vision-capable providers).
    Screenshot,
}

/// Short chit-chat with no task word: answered as plain chat even in control mode, so "하이"
/// does not trigger a page snapshot. ponytail: length + keyword heuristic; the model still
/// answers without actions for anything longer that needs none.
pub fn is_small_talk(text: &str) -> bool {
    const TASK: &[&str] = &[
        "요약",
        "정리",
        "번역",
        "찾",
        "검색",
        "열어",
        "열기",
        "눌러",
        "클릭",
        "이동",
        "가줘",
        "입력",
        "써줘",
        "채워",
        "추출",
        "복사",
        "비교",
        "읽어",
        "스크롤",
        "다운",
        "저장",
        "닫아",
        "탭",
        "페이지",
        "pdf",
        "표",
        "링크",
        "click",
        "open",
        "search",
        "find",
        "go ",
        "summar",
        "translate",
    ];
    let t = text.trim().to_lowercase();
    t.chars().count() <= 12 && !TASK.iter().any(|k| t.contains(k))
}

/// `요소 [10] 클릭` + the element's visible label → `"다음 페이지" 클릭`, so the action log
/// (shown to the user and fed back to the model) says what was actually clicked.
pub fn label_action_desc(desc: &str, label: &str) -> String {
    let label = label.trim();
    if label.is_empty() {
        return desc.to_string();
    }
    match desc.strip_prefix("요소 [").and_then(|r| r.split_once(']')) {
        Some((id, rest)) => format!("\"{label}\"[{id}]{rest}"),
        None => format!("{desc} · \"{label}\""),
    }
}

impl AgentAction {
    /// Short tool-style name for the activity cards (`click`, `type`, …).
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Navigate { .. } => "navigate",
            Self::Click { .. } | Self::ClickText { .. } => "click",
            Self::Type { .. } => "type",
            Self::Select { .. } => "select",
            Self::Hover { .. } => "hover",
            Self::Press { .. } => "press",
            Self::Scroll { .. } => "scroll",
            Self::OpenTab { .. } => "open_tab",
            Self::SwitchTab { .. } => "switch_tab",
            Self::CloseTab { .. } => "close_tab",
            Self::Extract { .. } => "extract",
            Self::Back => "back",
            Self::Forward => "forward",
            Self::Reload => "reload",
            Self::Wait { .. } | Self::WaitFor { .. } => "wait",
            Self::Done { .. } => "done",
            Self::FindInPage { .. } => "find_in_page",
            Self::ReadTab { .. } => "read_tab",
            Self::PdfPage { .. } => "pdf_page",
            Self::PdfSearch { .. } => "pdf_search",
            Self::FillForm { .. } => "fill_form",
            Self::BookmarksSearch { .. } => "bookmarks_search",
            Self::HistorySearch { .. } => "history_search",
            Self::Cite { .. } => "cite",
            Self::WaitForNetworkIdle { .. } => "wait_for_network_idle",
            Self::Screenshot => "screenshot",
        }
    }

    pub fn describe(&self) -> String {
        match self {
            Self::Navigate { url } => format!("이동 → {}", url.chars().take(42).collect::<String>()),
            Self::Click { target } => format!("요소 [{target}] 클릭"),
            Self::ClickText { text } => {
                format!("\"{}\" 텍스트 요소 클릭", text.chars().take(24).collect::<String>())
            }
            Self::Type {
                target, text, submit, ..
            } => {
                let preview: String = text.chars().take(24).collect();
                if *submit {
                    format!("요소 [{target}]에 \"{preview}\" 입력 후 제출")
                } else {
                    format!("요소 [{target}]에 \"{preview}\" 입력")
                }
            }
            Self::Select { target, value } => {
                let preview: String = value.chars().take(24).collect();
                format!("요소 [{target}]에서 \"{preview}\" 선택")
            }
            Self::Hover { target } => format!("요소 [{target}] 호버"),
            Self::Press { key } => format!("키 입력 ({key})"),
            Self::Scroll { direction, .. } => format!("스크롤 ({direction})"),
            Self::OpenTab { url } => format!("새 탭 열기 → {}", url.chars().take(40).collect::<String>()),
            Self::SwitchTab { tab } => format!("탭 [{tab}]으로 전환"),
            Self::CloseTab { tab } => format!("탭 [{tab}] 닫기"),
            Self::Extract { kind, copy } => {
                if *copy {
                    format!("데이터 추출({kind}) 및 복사")
                } else {
                    format!("데이터 추출({kind})")
                }
            }
            Self::Back => "뒤로 가기".to_string(),
            Self::Forward => "앞으로 가기".to_string(),
            Self::Reload => "새로고침".to_string(),
            Self::Wait { ms } => format!("{ms}ms 대기"),
            Self::WaitFor { text, .. } => {
                format!("\"{}\" 표시 대기", text.chars().take(24).collect::<String>())
            }
            Self::Done { .. } => "작업 완료".to_string(),
            Self::FindInPage { text } => format!("\"{}\" 찾기", text.chars().take(24).collect::<String>()),
            Self::ReadTab { tab } => format!("탭 [{tab}] 읽기"),
            Self::PdfPage { page } => format!("PDF {page}쪽 읽기"),
            Self::PdfSearch { text } => format!("PDF에서 \"{}\" 검색", text.chars().take(24).collect::<String>()),
            Self::FillForm { fields } => format!("입력란 {}개 채우기", fields.len()),
            Self::BookmarksSearch { query } => format!("북마크 검색: {}", query.chars().take(24).collect::<String>()),
            Self::HistorySearch { query } => format!("방문 기록 검색: {}", query.chars().take(24).collect::<String>()),
            Self::Cite { style } => format!("출처 인용문 ({style})"),
            Self::WaitForNetworkIdle { .. } => "로딩 완료 대기".to_string(),
            Self::Screenshot => "화면 캡처".to_string(),
        }
    }
}

/// Returns a minified JS helper call `hi(el)` that outlines `el` and flashes an "AI" badge.
pub(super) const AGENT_HIGHLIGHT_FN: &str = "function hi(el){try{el.scrollIntoView({block:'center',inline:'center'});var p=el.style.outline;el.style.outline='2px solid #4f63f6';var r=el.getBoundingClientRect();var b=document.createElement('div');b.textContent='AI';b.style.cssText='position:fixed;left:'+Math.max(4,r.left)+'px;top:'+Math.max(4,r.top-18)+'px;background:#4f63f6;color:#fff;font:bold 10px/14px sans-serif;padding:0 5px;border-radius:4px;z-index:2147483647;pointer-events:none;';document.body&&document.body.appendChild(b);setTimeout(function(){try{el.style.outline=p;b.remove();}catch(e){}},700);}catch(e){}}";

/// Minified JS helper `fireClick(el)` dispatching full pointer + mouse + click events.
pub(super) const AGENT_CLICK_FN: &str = "function fireClick(el){var w=(el.ownerDocument&&el.ownerDocument.defaultView)||window;var r=el.getBoundingClientRect();var o={bubbles:true,cancelable:true,view:w,clientX:r.left+r.width/2,clientY:r.top+r.height/2};try{el.dispatchEvent(new w.PointerEvent('pointerdown',o));}catch(e){}try{el.dispatchEvent(new w.MouseEvent('mousedown',o));}catch(e){}if(typeof el.focus==='function')el.focus();try{el.dispatchEvent(new w.PointerEvent('pointerup',o));}catch(e){}try{el.dispatchEvent(new w.MouseEvent('mouseup',o));}catch(e){}el.click();}";

pub fn build_agent_action_js(action: &AgentAction) -> Option<String> {
    match action {
        AgentAction::Click { target } => Some(format!(
            r#"(function(){{try{{
  {AGENT_HIGHLIGHT_FN}
  {AGENT_CLICK_FN}
  var el=(window.__rbAid&&window.__rbAid[{target}])||document.querySelector('[data-rb-aid="{target}"]');
  if(!el)return JSON.stringify({{ok:false,msg:'요소 [{target}]를 찾을 수 없습니다'}});
  var prevExp=el.getAttribute('aria-expanded');var prevChk=el.checked;
  hi(el);fireClick(el);
  var extra='';
  if(el.checked!==undefined&&el.checked!==prevChk)extra=' checked='+el.checked;
  else if(prevExp!==null&&el.getAttribute('aria-expanded')!==prevExp)extra=' expanded='+el.getAttribute('aria-expanded');
  function lbl(el){{try{{return String(el.innerText||el.value||el.getAttribute('aria-label')||el.placeholder||el.title||el.name||'').replace(/\s+/g,' ').trim().slice(0,40);}}catch(e){{return '';}}}}
  return JSON.stringify({{ok:true,msg:'clicked [{target}]'+extra,label:lbl(el)}});
}}catch(e){{return JSON.stringify({{ok:false,msg:String(e)}});}}}})()"#
        )),
        AgentAction::ClickText { text } => {
            let text_js = serde_json::to_string(text).unwrap_or_else(|_| "\"\"".to_string());
            Some(format!(
                r#"(function(){{try{{
  {AGENT_HIGHLIGHT_FN}
  {AGENT_CLICK_FN}
  var want={text_js}.replace(/\s+/g,' ').trim().toLowerCase();
  if(!want)return JSON.stringify({{ok:false,msg:'빈 텍스트입니다'}});
  var exact=null,part=null;
  var sel='a,button,summary,label,[role="button"],[role="link"],[role="tab"],[role="menuitem"],[role="option"],input[type="submit"],input[type="button"],div,span,li';
  function search(doc,depth){{
    if(!doc||exact||depth>4)return;
    var list;try{{list=doc.querySelectorAll(sel);}}catch(e){{return;}}
    for(var i=0;i<list.length;i++){{
      var el=list[i];var tag=el.tagName;
      if((tag==='DIV'||tag==='SPAN'||tag==='LI')&&!el.getAttribute('role')){{
        var cs;try{{cs=getComputedStyle(el);}}catch(e){{continue;}}
        if(cs.cursor!=='pointer')continue;
      }}
      var cs2;try{{cs2=getComputedStyle(el);}}catch(e){{continue;}}
      if(cs2.display==='none'||cs2.visibility==='hidden'||parseFloat(cs2.opacity)===0)continue;
      var r=el.getBoundingClientRect();if(r.width<4||r.height<4)continue;
      var txt=String(el.innerText||el.value||el.getAttribute('aria-label')||el.title||'').replace(/\s+/g,' ').trim().toLowerCase();
      if(!txt)continue;
      if(txt===want){{exact=el;return;}}
      if(!part&&txt.indexOf(want)>=0&&txt.length<=80)part=el;
    }}
    var all;try{{all=doc.querySelectorAll('*');}}catch(e){{return;}}
    for(var k=0;k<all.length&&!exact;k++){{
      if(all[k].shadowRoot)search(all[k].shadowRoot,depth+1);
      if(all[k].tagName==='IFRAME'){{try{{if(all[k].contentDocument)search(all[k].contentDocument,depth+1);}}catch(e){{}}}}
    }}
  }}
  search(document,0);
  var hit=exact||part;
  if(!hit)return JSON.stringify({{ok:false,msg:'"'+want+'" 요소를 찾을 수 없습니다'}});
  hi(hit);fireClick(hit);
  return JSON.stringify({{ok:true,msg:'clicked text '+want}});
}}catch(e){{return JSON.stringify({{ok:false,msg:String(e)}});}}}})()"#
            ))
        }
        AgentAction::Type {
            target,
            text,
            submit,
            clear,
        } => {
            let text_js = serde_json::to_string(text).unwrap_or_else(|_| "\"\"".to_string());
            let submit_js = if *submit { "true" } else { "false" };
            let clear_js = if *clear { "true" } else { "false" };
            Some(format!(
                r#"(function(){{try{{
  {AGENT_HIGHLIGHT_FN}
  var el=(window.__rbAid&&window.__rbAid[{target}])||document.querySelector('[data-rb-aid="{target}"]');
  if(!el)return JSON.stringify({{ok:false,msg:'요소 [{target}]를 찾을 수 없습니다'}});
  hi(el);
  if(typeof el.focus==='function')el.focus();
  var doc=el.ownerDocument||document;
  var win=doc.defaultView||window;
  var val={text_js};
  if(el.isContentEditable){{
    el.click();
    if({clear_js}){{
      if(!doc.execCommand||!doc.execCommand('selectAll',false,null)||!doc.execCommand('insertText',false,val)){{
        el.textContent=val;
        el.dispatchEvent(new win.InputEvent('input',{{bubbles:true,data:val,inputType:'insertText'}}));
      }}
    }}else{{
      var appended=(el.textContent||'')+val;
      if(!doc.execCommand||!doc.execCommand('insertText',false,val)){{
        el.textContent=appended;
        el.dispatchEvent(new win.InputEvent('input',{{bubbles:true,data:val,inputType:'insertText'}}));
      }}
    }}
  }}else{{
    var newVal={clear_js}?val:(String(el.value||'')+val);
    var proto=el.tagName==='TEXTAREA'?win.HTMLTextAreaElement.prototype:win.HTMLInputElement.prototype;
    var desc=proto?Object.getOwnPropertyDescriptor(proto,'value'):null;
    if(desc&&desc.set)desc.set.call(el,newVal);else el.value=newVal;
    el.dispatchEvent(new win.Event('input',{{bubbles:true}}));
    el.dispatchEvent(new win.Event('change',{{bubbles:true}}));
  }}
  if({submit_js}){{
    var ko={{key:'Enter',code:'Enter',keyCode:13,which:13,bubbles:true,cancelable:true,isComposing:false}};
    el.dispatchEvent(new win.KeyboardEvent('keydown',ko));
    el.dispatchEvent(new win.KeyboardEvent('keypress',ko));
    el.dispatchEvent(new win.KeyboardEvent('keyup',ko));
    if(el.form&&typeof el.form.requestSubmit==='function')el.form.requestSubmit();
    else if(el.form)el.form.submit();
  }}
  function lbl(el){{try{{return String(el.innerText||el.value||el.getAttribute('aria-label')||el.placeholder||el.title||el.name||'').replace(/\s+/g,' ').trim().slice(0,40);}}catch(e){{return '';}}}}
  return JSON.stringify({{ok:true,msg:'typed into [{target}]',label:lbl(el)}});
}}catch(e){{return JSON.stringify({{ok:false,msg:String(e)}});}}}})()"#
            ))
        }
        AgentAction::Select { target, value } => {
            let val_js = serde_json::to_string(value).unwrap_or_else(|_| "\"\"".to_string());
            Some(format!(
                r#"(function(){{try{{
  {AGENT_HIGHLIGHT_FN}
  var el=(window.__rbAid&&window.__rbAid[{target}])||document.querySelector('[data-rb-aid="{target}"]');
  if(!el||el.tagName!=='SELECT')return JSON.stringify({{ok:false,msg:'요소 [{target}]는 <select>가 아닙니다'}});
  hi(el);
  var win=(el.ownerDocument&&el.ownerDocument.defaultView)||window;
  var want={val_js};var wl=want.toLowerCase();var idx=-1;
  for(var i=0;i<el.options.length;i++){{
    var o=el.options[i];
    if(o.value===want||o.text.trim()===want){{idx=i;break;}}
  }}
  if(idx<0){{
    for(var i=0;i<el.options.length;i++){{
      var t=el.options[i];
      if(t.value.toLowerCase().indexOf(wl)>=0||t.text.toLowerCase().indexOf(wl)>=0){{idx=i;break;}}
    }}
  }}
  if(idx<0)return JSON.stringify({{ok:false,msg:'옵션 "'+want+'"을 찾을 수 없습니다'}});
  el.selectedIndex=idx;
  el.dispatchEvent(new win.Event('input',{{bubbles:true}}));
  el.dispatchEvent(new win.Event('change',{{bubbles:true}}));
  return JSON.stringify({{ok:true,msg:'selected '+el.options[idx].text.trim()}});
}}catch(e){{return JSON.stringify({{ok:false,msg:String(e)}});}}}})()"#
            ))
        }
        AgentAction::Hover { target } => Some(format!(
            r#"(function(){{try{{
  {AGENT_HIGHLIGHT_FN}
  var el=(window.__rbAid&&window.__rbAid[{target}])||document.querySelector('[data-rb-aid="{target}"]');
  if(!el)return JSON.stringify({{ok:false,msg:'요소 [{target}]를 찾을 수 없습니다'}});
  hi(el);
  var w=(el.ownerDocument&&el.ownerDocument.defaultView)||window;
  var r=el.getBoundingClientRect();
  var opt={{bubbles:true,cancelable:true,view:w,clientX:r.left+r.width/2,clientY:r.top+r.height/2}};
  el.dispatchEvent(new w.PointerEvent('pointerover',opt));
  el.dispatchEvent(new w.PointerEvent('pointerenter',{{bubbles:false,clientX:opt.clientX,clientY:opt.clientY}}));
  el.dispatchEvent(new w.MouseEvent('mouseover',opt));
  el.dispatchEvent(new w.MouseEvent('mouseenter',{{bubbles:false,clientX:opt.clientX,clientY:opt.clientY}}));
  el.dispatchEvent(new w.MouseEvent('mousemove',opt));
  function lbl(el){{try{{return String(el.innerText||el.value||el.getAttribute('aria-label')||el.placeholder||el.title||el.name||'').replace(/\s+/g,' ').trim().slice(0,40);}}catch(e){{return '';}}}}
  return JSON.stringify({{ok:true,msg:'hovered [{target}]',label:lbl(el)}});
}}catch(e){{return JSON.stringify({{ok:false,msg:String(e)}});}}}})()"#
        )),
        AgentAction::Press { key } => {
            let key_js = serde_json::to_string(key).unwrap_or_else(|_| "\"Enter\"".to_string());
            Some(format!(
                r#"(function(){{try{{
  var k={key_js};
  var t=document.activeElement||document.body;
  var w=(t&&t.ownerDocument&&t.ownerDocument.defaultView)||window;
  var code=k==='Enter'?13:k==='Escape'?27:k==='Tab'?9:0;
  var opt={{key:k,code:k,keyCode:code,which:code,bubbles:true,cancelable:true}};
  t.dispatchEvent(new w.KeyboardEvent('keydown',opt));
  t.dispatchEvent(new w.KeyboardEvent('keypress',opt));
  t.dispatchEvent(new w.KeyboardEvent('keyup',opt));
  if(k==='Enter'&&t&&t.form){{
    if(typeof t.form.requestSubmit==='function')t.form.requestSubmit();
    else t.form.submit();
  }}
  return JSON.stringify({{ok:true,msg:'pressed '+k}});
}}catch(e){{return JSON.stringify({{ok:false,msg:String(e)}});}}}})()"#
            ))
        }
        AgentAction::Scroll {
            direction,
            amount,
            target,
        } => {
            let amt = (*amount).clamp(1, 3000);
            let dir_lc = direction.trim().to_ascii_lowercase();
            let (dx, dy, mode) = match dir_lc.as_str() {
                "up" => (0, -amt, "by"),
                "left" => (-amt, 0, "by"),
                "right" => (amt, 0, "by"),
                "top" => (0, 0, "top"),
                "bottom" => (0, 0, "bottom"),
                _ => (0, amt, "by"),
            };
            let target_js = target.map_or_else(|| "null".to_string(), |id| id.to_string());
            Some(format!(
                r#"(function(){{try{{
  var tid={target_js};var mode='{mode}';var scroller=null;
  if(tid!==null){{
    var el=(window.__rbAid&&window.__rbAid[tid])||document.querySelector('[data-rb-aid="'+tid+'"]');
    while(el){{
      if(el.scrollHeight>el.clientHeight+4||el.scrollWidth>el.clientWidth+4){{scroller=el;break;}}
      el=el.parentElement;
    }}
  }}
  var se=document.scrollingElement||document.documentElement||document.body;
  if(!scroller&&se&&se.scrollHeight<=se.clientHeight+20&&se.scrollWidth<=se.clientWidth+20){{
    var bestArea=0;var alls=document.querySelectorAll('main,div,section,aside');
    for(var i=0;i<alls.length;i++){{
      var a=alls[i];
      if(a.scrollHeight>a.clientHeight+20){{
        var cs=getComputedStyle(a);
        if(cs.overflowY==='auto'||cs.overflowY==='scroll'){{
          var ar=a.clientWidth*a.clientHeight;
          if(ar>bestArea){{bestArea=ar;scroller=a;}}
        }}
      }}
    }}
  }}
  if(scroller){{
    if(mode==='top')scroller.scrollTo({{top:0,behavior:'smooth'}});
    else if(mode==='bottom')scroller.scrollTo({{top:scroller.scrollHeight,behavior:'smooth'}});
    else scroller.scrollBy({dx},{dy});
    var ms=Math.max(1,scroller.scrollHeight-scroller.clientHeight);
    var pct=Math.min(100,Math.round(scroller.scrollTop*100/ms));
    return JSON.stringify({{ok:true,msg:'scrolled container '+pct+'%'}});
  }}else{{
    if(mode==='top')window.scrollTo({{top:0,behavior:'smooth'}});
    else if(mode==='bottom')window.scrollTo({{top:se?se.scrollHeight:999999,behavior:'smooth'}});
    else window.scrollBy({dx},{dy});
    var sy=se?Math.round(se.scrollTop||window.scrollY||0):0;
    var ms2=se?Math.max(1,se.scrollHeight-(se.clientHeight||window.innerHeight||800)):1;
    var pct2=Math.min(100,Math.round(sy*100/ms2));
    return JSON.stringify({{ok:true,msg:'scrolled page '+pct2+'%'}});
  }}
}}catch(e){{return JSON.stringify({{ok:false,msg:String(e)}});}}}})()"#
            ))
        }
        _ => None,
    }
}

pub fn build_agent_text_check_js(text: &str) -> String {
    let text_js = serde_json::to_string(text.trim()).unwrap_or_else(|_| "\"\"".to_string());
    format!(
        r#"(function(){{try{{
  var want={text_js}.toLowerCase();
  if(!want)return JSON.stringify({{found:true}});
  function hasText(doc,depth){{
    if(!doc||depth>4)return false;
    var body=doc.body||doc;
    var txt=String((body.innerText||body.textContent||'')).toLowerCase();
    if(txt.indexOf(want)>=0)return true;
    var all;try{{all=doc.querySelectorAll('*');}}catch(e){{return false;}}
    for(var i=0;i<all.length;i++){{
      if(all[i].shadowRoot&&hasText(all[i].shadowRoot,depth+1))return true;
      if(all[i].tagName==='IFRAME'){{try{{if(all[i].contentDocument&&hasText(all[i].contentDocument,depth+1))return true;}}catch(e){{}}}}
    }}
    return false;
  }}
  return JSON.stringify({{found:hasText(document,0)}});
}}catch(e){{return JSON.stringify({{found:false}});}}}})()"#
    )
}

pub fn build_agent_extract_js(kind: &str) -> String {
    let kind_js =
        serde_json::to_string(kind.trim().to_ascii_lowercase().as_str()).unwrap_or_else(|_| "\"tables\"".to_string());
    format!(
        r#"(function(){{try{{
  var kind={kind_js};
  var docs=[];
  function collectDocs(doc,depth){{
    if(!doc||depth>4)return;
    docs.push(doc);
    var all;try{{all=doc.querySelectorAll('*');}}catch(e){{return;}}
    for(var i=0;i<all.length;i++){{
      var h=all[i];
      if(h.shadowRoot)collectDocs(h.shadowRoot,depth+1);
      if(h.tagName==='IFRAME'){{try{{if(h.contentDocument)collectDocs(h.contentDocument,depth+1);}}catch(e){{}}}}
    }}
  }}
  collectDocs(document,0);
  function extractTables(){{
    var out=[];
    for(var d=0;d<docs.length;d++){{
      var tbls=docs[d].querySelectorAll('table');
      for(var ti=0;ti<tbls.length;ti++){{
        var tbl=tbls[ti];var cs;try{{cs=getComputedStyle(tbl);}}catch(e){{cs=null;}}
        if(cs&&(cs.display==='none'||cs.visibility==='hidden'))continue;
        var rows=tbl.querySelectorAll('tr');
        var lines=[];
        for(var ri=0;ri<rows.length&&lines.length<80;ri++){{
          var cells=rows[ri].querySelectorAll('th,td');
          if(!cells.length)continue;
          var cols=[];
          for(var ci=0;ci<cells.length;ci++){{
            cols.push(String(cells[ci].innerText||'').replace(/\s+/g,' ').replace(/\|/g,'/').trim().slice(0,120));
          }}
          lines.push('| '+cols.join(' | ')+' |');
          if(lines.length===1){{lines.push('| '+cols.map(function(){{return '---';}}).join(' | ')+' |');}}
        }}
        if(lines.length>=2)out.push(lines.join('\n'));
      }}
    }}
    return out.length?out.join('\n\n'):'(표 없음)';
  }}
  function extractLinks(){{
    var seen=Object.create(null);var out=[];
    for(var d=0;d<docs.length&&out.length<80;d++){{
      var as=docs[d].querySelectorAll('a[href]');
      for(var i=0;i<as.length&&out.length<80;i++){{
        var a=as[i];var href=a.href||a.getAttribute('href')||'';
        if(!href||href.indexOf('javascript:')===0||seen[href])continue;
        var txt=String(a.innerText||a.getAttribute('aria-label')||a.title||'').replace(/\s+/g,' ').trim().slice(0,80);
        if(!txt)continue;
        seen[href]=1;
        out.push('- ['+txt+']('+href+')');
      }}
    }}
    return out.length?out.join('\n'):'(링크 없음)';
  }}
  var data='';
  if(kind==='links')data=extractLinks();
  else if(kind==='text')data=(document.body?document.body.innerText:'');
  else if(kind==='all')data=extractTables()+'\n\n'+(document.body?document.body.innerText:'');
  else data=extractTables();
  return JSON.stringify({{ok:true,data:String(data||'').slice(0,12000)}});
}}catch(e){{return JSON.stringify({{ok:false,data:'',msg:String(e)}});}}}})()"#
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentPlan {
    pub actions: Vec<AgentAction>,
    pub finish_after: bool,
    pub summary: String,
    pub note: String,
}

pub(super) fn find_closing_fence(s: &str) -> Option<usize> {
    let bytes = s.as_bytes();
    let mut in_str = false;
    let mut escaped = false;
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if in_str {
            if escaped {
                escaped = false;
            } else if b == b'\\' {
                escaped = true;
            } else if b == b'"' {
                in_str = false;
            }
            i += 1;
            continue;
        }
        if b == b'"' {
            in_str = true;
            i += 1;
            continue;
        }
        if s[i..].starts_with("```") {
            return Some(i);
        }
        i += 1;
    }
    None
}

pub fn parse_agent_response(reply: &str) -> (String, Option<AgentPlan>) {
    let mut best_span: Option<(usize, usize, AgentPlan)> = None;
    let mut offset = 0usize;
    while let Some(rel_start) = reply[offset..].find("```") {
        let fence_start = offset + rel_start;
        let after_ticks = &reply[fence_start + 3..];
        let Some(nl_rel) = after_ticks.find('\n') else {
            break;
        };
        let lang = after_ticks[..nl_rel].trim().to_ascii_lowercase();
        let body_start = fence_start + 3 + nl_rel + 1;
        let Some(end_rel) = find_closing_fence(&reply[body_start..]) else {
            break;
        };
        let body = reply[body_start..body_start + end_rel].trim();
        let fence_end = body_start + end_rel + 3;
        if lang == "browser-action"
            && let Some(plan) = parse_action_plan_json(body)
        {
            best_span = Some((fence_start, fence_end, plan));
            break;
        }
        offset = fence_end;
    }

    if let Some((start, end, plan)) = best_span {
        let before = reply[..start].trim_end();
        let after = reply[end..].trim_start();
        let cleaned = match (before.is_empty(), after.is_empty()) {
            (true, true) => String::new(),
            (false, true) => before.to_string(),
            (true, false) => after.to_string(),
            (false, false) => format!("{before}\n\n{after}"),
        };
        return (cleaned, Some(plan));
    }

    if let Some(plan) = parse_action_plan_json(reply.trim()) {
        return (String::new(), Some(plan));
    }
    (reply.to_string(), None)
}

pub(super) fn parse_action_plan_json(raw: &str) -> Option<AgentPlan> {
    let val: Value = serde_json::from_str(raw).ok()?;
    let top_done = val.get("done").and_then(|v| v.as_bool()).unwrap_or(false);
    let top_summary = val
        .get("summary")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    let note = val
        .get("note")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();

    if let Some(arr) = val.get("actions").and_then(|v| v.as_array()) {
        let mut actions = Vec::new();
        let mut finish_after = top_done;
        let mut summary = top_summary;
        for item in arr.iter().take(6) {
            if let Some(act) = parse_action_value(item) {
                match act {
                    AgentAction::Done { summary: s } => {
                        finish_after = true;
                        if !s.is_empty() {
                            summary = s;
                        }
                        break;
                    }
                    other => actions.push(other),
                }
            }
        }
        if actions.is_empty() && !finish_after {
            return None;
        }
        return Some(AgentPlan {
            actions,
            finish_after,
            summary,
            note,
        });
    }

    let act = parse_action_value(&val)?;
    match act {
        AgentAction::Done { summary: s } => Some(AgentPlan {
            actions: Vec::new(),
            finish_after: true,
            summary: if !s.is_empty() { s } else { top_summary },
            note,
        }),
        other => Some(AgentPlan {
            actions: vec![other],
            finish_after: top_done,
            summary: top_summary,
            note,
        }),
    }
}

pub(super) fn parse_target_id(val: &Value) -> Option<usize> {
    val.get("target")
        .or_else(|| val.get("id"))
        .or_else(|| val.get("element_index"))
        .and_then(|v| v.as_u64())
        .map(|n| n as usize)
}

pub(super) fn parse_action_value(val: &Value) -> Option<AgentAction> {
    let act = val.get("action")?.as_str()?.trim().to_ascii_lowercase();
    match act.as_str() {
        "navigate" => Some(AgentAction::Navigate {
            url: val.get("url")?.as_str()?.trim().to_string(),
        }),
        "click" => Some(AgentAction::Click {
            target: parse_target_id(val)?,
        }),
        "click_text" => {
            let text = val.get("text")?.as_str()?.trim().to_string();
            if text.is_empty() {
                return None;
            }
            Some(AgentAction::ClickText { text })
        }
        "type" => Some(AgentAction::Type {
            target: parse_target_id(val)?,
            text: val.get("text")?.as_str()?.to_string(),
            submit: val.get("submit").and_then(|v| v.as_bool()).unwrap_or(false),
            clear: val.get("clear").and_then(|v| v.as_bool()).unwrap_or(true),
        }),
        "select" => Some(AgentAction::Select {
            target: parse_target_id(val)?,
            value: val.get("value")?.as_str()?.to_string(),
        }),
        "hover" => Some(AgentAction::Hover {
            target: parse_target_id(val)?,
        }),
        "press" => Some(AgentAction::Press {
            key: val.get("key")?.as_str()?.to_string(),
        }),
        "scroll" => Some(AgentAction::Scroll {
            direction: val
                .get("direction")
                .and_then(|v| v.as_str())
                .unwrap_or("down")
                .to_string(),
            amount: val.get("amount").and_then(|v| v.as_i64()).unwrap_or(520) as i32,
            target: val.get("target").and_then(|v| v.as_u64()).map(|n| n as usize),
        }),
        "open_tab" => Some(AgentAction::OpenTab {
            url: val.get("url")?.as_str()?.trim().to_string(),
        }),
        "switch_tab" => Some(AgentAction::SwitchTab {
            tab: val.get("tab")?.as_u64()? as usize,
        }),
        "close_tab" => Some(AgentAction::CloseTab {
            tab: val.get("tab")?.as_u64()? as usize,
        }),
        "extract" => Some(AgentAction::Extract {
            kind: val
                .get("kind")
                .and_then(|v| v.as_str())
                .unwrap_or("tables")
                .trim()
                .to_ascii_lowercase(),
            copy: val.get("copy").and_then(|v| v.as_bool()).unwrap_or(false),
        }),
        "back" => Some(AgentAction::Back),
        "forward" => Some(AgentAction::Forward),
        "reload" => Some(AgentAction::Reload),
        "wait" => Some(AgentAction::Wait {
            ms: val.get("ms").and_then(|v| v.as_u64()).unwrap_or(1000).clamp(100, 5000),
        }),
        "wait_for" => {
            let text = val.get("text")?.as_str()?.trim().to_string();
            if text.is_empty() {
                return None;
            }
            Some(AgentAction::WaitFor {
                text,
                timeout_ms: val
                    .get("timeout_ms")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(5000)
                    .clamp(300, 8000),
            })
        }
        "find_in_page" | "find" => {
            let text = val
                .get("text")
                .or_else(|| val.get("query"))?
                .as_str()?
                .trim()
                .to_string();
            (!text.is_empty()).then_some(AgentAction::FindInPage { text })
        }
        "read_tab" => Some(AgentAction::ReadTab {
            tab: val.get("tab")?.as_u64()? as usize,
        }),
        "pdf_page" => Some(AgentAction::PdfPage {
            page: val.get("page")?.as_u64()?.clamp(1, 100_000) as u32,
        }),
        "pdf_search" => {
            let text = val
                .get("text")
                .or_else(|| val.get("query"))?
                .as_str()?
                .trim()
                .to_string();
            (!text.is_empty()).then_some(AgentAction::PdfSearch { text })
        }
        "fill_form" => {
            let fields: Vec<(usize, String)> = val
                .get("fields")?
                .as_array()?
                .iter()
                .filter_map(|f| {
                    let value = f.get("value").or_else(|| f.get("text"))?;
                    let value = value.as_str().map(str::to_string).unwrap_or_else(|| value.to_string());
                    Some((parse_target_id(f)?, value))
                })
                .take(30)
                .collect();
            (!fields.is_empty()).then_some(AgentAction::FillForm { fields })
        }
        "bookmarks_search" | "bookmark_search" => Some(AgentAction::BookmarksSearch {
            query: val.get("query")?.as_str()?.trim().to_string(),
        }),
        "history_search" => Some(AgentAction::HistorySearch {
            query: val.get("query")?.as_str()?.trim().to_string(),
        }),
        "cite" => Some(AgentAction::Cite {
            style: val
                .get("style")
                .and_then(|v| v.as_str())
                .unwrap_or("apa")
                .trim()
                .to_ascii_lowercase(),
        }),
        "wait_for_network_idle" | "wait_idle" => Some(AgentAction::WaitForNetworkIdle {
            timeout_ms: val
                .get("timeout_ms")
                .and_then(|v| v.as_u64())
                .unwrap_or(6000)
                .clamp(500, 15_000),
        }),
        "screenshot" => Some(AgentAction::Screenshot),
        "done" => Some(AgentAction::Done {
            summary: val
                .get("summary")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim()
                .to_string(),
        }),
        _ => None,
    }
}

#[cfg(test)]
mod label_tests {
    #[test]
    fn small_talk_skips_the_agent() {
        use super::is_small_talk as s;
        for t in ["하이", "안녕!", "고마워 ㅎㅎ", "hi", "ok", "ㅋㅋㅋ"] {
            assert!(s(t), "{t}");
        }
        for t in [
            "요약해줘",
            "다음 눌러",
            "이 표 추출",
            "search rust",
            "로그인 버튼 클릭해 줘 그리고 메일함 열어줘",
        ] {
            assert!(!s(t), "{t}");
        }
    }

    #[test]
    fn labels_replace_element_numbers() {
        use super::label_action_desc as l;
        assert_eq!(l("요소 [10] 클릭", "다음 페이지"), "\"다음 페이지\"[10] 클릭");
        assert_eq!(l("요소 [3]에 \"rust\" 입력", "검색"), "\"검색\"[3]에 \"rust\" 입력");
        assert_eq!(l("요소 [10] 클릭", "  "), "요소 [10] 클릭");
        assert_eq!(l("스크롤 (down)", "x"), "스크롤 (down) · \"x\"");
    }
}
