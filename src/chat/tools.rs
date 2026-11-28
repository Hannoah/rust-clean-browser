//! Page-side scripts for the browser agent's data tools (find_in_page, pdf_page, pdf_search,
//! fill_form, wait_for_network_idle). Each returns `{ok,data,msg}` or, while async work is still
//! running, `{pending:true}` (the app re-runs the same script until it settles or times out).

fn js_str(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".to_string())
}

/// Finds `text` in the page (and same-origin frames / shadow roots), scrolls to and outlines the
/// first hit, and returns the match count with a few surrounding snippets.
pub fn find_in_page_js(text: &str) -> String {
    let want = js_str(text.trim());
    format!(
        r#"(function(){{try{{
  {hl}
  var want={want}.toLowerCase();if(!want)return JSON.stringify({{ok:false,msg:'빈 검색어'}});
  var hits=[];
  function scan(root,depth){{
    if(!root||depth>4)return;
    var w;try{{w=(root.ownerDocument||root).createTreeWalker(root,NodeFilter.SHOW_TEXT);}}catch(e){{return;}}
    var n;while((n=w.nextNode())&&hits.length<200){{
      var t=n.nodeValue;if(!t)continue;var lt=t.toLowerCase();var i=lt.indexOf(want);
      while(i>=0&&hits.length<200){{hits.push({{n:n,i:i}});i=lt.indexOf(want,i+want.length);}}
    }}
    var all;try{{all=root.querySelectorAll('*');}}catch(e){{return;}}
    for(var k=0;k<all.length;k++){{
      if(all[k].shadowRoot)scan(all[k].shadowRoot,depth+1);
      if(all[k].tagName==='IFRAME'){{try{{if(all[k].contentDocument)scan(all[k].contentDocument.body,depth+1);}}catch(e){{}}}}
    }}
  }}
  scan(document.body,0);
  var vis=hits.filter(function(h){{var el=h.n.parentElement;if(!el)return false;var r=el.getBoundingClientRect();return r.width>0&&r.height>0;}});
  if(!vis.length)return JSON.stringify({{ok:false,msg:'"'+{want}+'" 없음'}});
  var first=vis[0].n.parentElement;first.scrollIntoView({{block:'center'}});hi(first);
  var snips=vis.slice(0,5).map(function(h,idx){{
    var t=h.n.nodeValue;var s=Math.max(0,h.i-60);
    return (idx+1)+'. …'+t.slice(s,h.i+want.length+60).replace(/\s+/g,' ').trim()+'…';
  }});
  return JSON.stringify({{ok:true,msg:vis.length+'곳 일치, 첫 위치로 이동',data:'"'+{want}+'" '+vis.length+'곳 일치 (첫 위치로 스크롤함)\n'+snips.join('\n')}});
}}catch(e){{return JSON.stringify({{ok:false,msg:String(e)}});}}}})()"#,
        hl = super::agent::AGENT_HIGHLIGHT_FN,
    )
}

/// Shared prologue for the pdf.js viewer tools: one async job per `nonce`, polled until done.
fn pdf_job(nonce: u64, start: &str) -> String {
    format!(
        r#"(function(){{try{{
  var A=window.PDFViewerApplication;
  if(!A||!A.pdfDocument)return JSON.stringify({{ok:false,msg:'PDF 뷰어가 열린 탭이 아닙니다'}});
  var key='rb{nonce}';var J=window.__rbJob;
  if(!J||J.key!==key){{J=window.__rbJob={{key:key,done:false}};
    try{{ {start} }}catch(e){{J.err=String(e);J.done=true;}}
  }}
  if(!J.done)return JSON.stringify({{pending:true}});
  window.__rbJob=null;
  if(J.err)return JSON.stringify({{ok:false,msg:J.err}});
  return JSON.stringify({{ok:true,msg:J.msg||'',data:String(J.data||'').slice(0,12000)}});
}}catch(e){{return JSON.stringify({{ok:false,msg:String(e)}});}}}})()"#
    )
}

const PDF_PAGE_TEXT: &str = "function pageText(n){return A.pdfDocument.getPage(n).then(function(p){return p.getTextContent();}).then(function(tc){return tc.items.map(function(i){return i.str+(i.hasEOL?'\\n':'');}).join('');});}";

/// Shows page `page` of the open PDF and returns its text.
pub fn pdf_page_js(page: u32, nonce: u64) -> String {
    pdf_job(
        nonce,
        &format!(
            r#"{PDF_PAGE_TEXT}
    var n={page};var total=A.pagesCount;
    if(n<1||n>total){{J.err='페이지 범위는 1-'+total+'입니다';J.done=true;}}
    else{{A.page=n;pageText(n).then(function(t){{J.data='[p.'+n+'/'+total+']\n'+t;J.msg='p.'+n+'/'+total;J.done=true;}},function(e){{J.err=String(e);J.done=true;}});}}"#
        ),
    )
}

/// Searches every page of the open PDF, jumps to the first match, and lists matches with context.
pub fn pdf_search_js(text: &str, nonce: u64) -> String {
    let want = js_str(text.trim());
    pdf_job(
        nonce,
        &format!(
            r#"{PDF_PAGE_TEXT}
    var want={want}.toLowerCase();var total=A.pagesCount;var out=[];var pages=[];
    if(!want){{J.err='빈 검색어';J.done=true;}}
    else{{(function next(n){{
      if(n>total||out.length>=20){{
        if(pages.length)A.page=pages[0];
        J.msg=pages.length?pages.length+'쪽에서 발견':'없음';
        J.data='"'+{want}+'": '+(pages.length?pages.length+'쪽에서 발견 (p.'+pages.join(', ')+')\n'+out.join('\n'):'PDF에 없음');
        J.done=true;return;
      }}
      pageText(n).then(function(t){{
        var lt=t.toLowerCase();var i=lt.indexOf(want);if(i>=0)pages.push(n);
        while(i>=0&&out.length<20){{out.push('p.'+n+': …'+t.slice(Math.max(0,i-60),i+want.length+60).replace(/\s+/g,' ')+'…');i=lt.indexOf(want,i+want.length);}}
        next(n+1);
      }},function(e){{J.err=String(e);J.done=true;}});
    }})(1);}}"#
        ),
    )
}

/// Fills several form fields at once (text inputs, textareas, selects by value or label,
/// checkboxes/radios by truthy text). Never submits.
pub fn fill_form_js(fields: &[(usize, String)]) -> String {
    let pairs: Vec<String> = fields.iter().map(|(t, v)| format!("[{t},{}]", js_str(v))).collect();
    format!(
        r#"(function(){{try{{
  {hl}
  var fields=[{pairs}];var done=[],miss=[];
  function setVal(el,v){{
    var proto=el.tagName==='TEXTAREA'?HTMLTextAreaElement.prototype:HTMLInputElement.prototype;
    var d=Object.getOwnPropertyDescriptor(proto,'value');
    if(d&&d.set)d.set.call(el,v);else el.value=v;
  }}
  for(var i=0;i<fields.length;i++){{
    var id=fields[i][0],v=fields[i][1];
    var el=(window.__rbAid&&window.__rbAid[id])||document.querySelector('[data-rb-aid="'+id+'"]');
    if(!el){{miss.push(id);continue;}}
    hi(el);
    var tag=el.tagName,type=(el.type||'').toLowerCase();
    if(tag==='SELECT'){{
      var lv=String(v).toLowerCase(),hit=false;
      for(var o=0;o<el.options.length;o++){{var op=el.options[o];
        if(op.value.toLowerCase()===lv||op.text.trim().toLowerCase()===lv){{el.selectedIndex=o;hit=true;break;}}}}
      if(!hit){{miss.push(id);continue;}}
    }}else if(type==='checkbox'||type==='radio'){{
      el.checked=!/^(false|0|no|off|아니오|해제)$/i.test(String(v).trim());
    }}else if(el.isContentEditable){{el.focus();el.textContent=v;}}
    else{{el.focus();setVal(el,v);}}
    el.dispatchEvent(new Event('input',{{bubbles:true}}));el.dispatchEvent(new Event('change',{{bubbles:true}}));
    done.push(id);
  }}
  var msg=done.length+'개 입력'+(miss.length?', 실패 ['+miss.join(',')+']':'');
  return JSON.stringify({{ok:done.length>0,msg:msg,data:''}});
}}catch(e){{return JSON.stringify({{ok:false,msg:String(e)}});}}}})()"#,
        hl = super::agent::AGENT_HIGHLIGHT_FN,
        pairs = pairs.join(","),
    )
}

/// Settles once the document is complete and no new network resources finished for ~700ms.
/// ponytail: resource-timing heuristic (in-flight requests are invisible to it); hook fetch/XHR
/// if long-polling pages need an exact count.
pub fn network_idle_js(nonce: u64) -> String {
    format!(
        r#"(function(){{try{{
  var key='rb{nonce}';var S=window.__rbIdle;var now=Date.now();
  var n=performance.getEntriesByType('resource').length;
  if(!S||S.key!==key){{window.__rbIdle={{key:key,n:n,t:now,t0:now}};return JSON.stringify({{pending:true}});}}
  if(n!==S.n){{S.n=n;S.t=now;}}
  if(document.readyState!=='complete'||now-S.t<700)return JSON.stringify({{pending:true}});
  window.__rbIdle=null;
  return JSON.stringify({{ok:true,msg:'네트워크 대기 완료 ('+(now-S.t0)+'ms)',data:''}});
}}catch(e){{return JSON.stringify({{ok:false,msg:String(e)}});}}}})()"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The scripts are format!-built JS: catch brace/quote slips by checking they balance and
    /// that user text is embedded as a JSON string (no injection through quotes).
    #[test]
    fn scripts_are_balanced_and_escape_input() {
        // Balance is checked with bracket-free input (brackets inside user text are data).
        for js in [
            find_in_page_js("plain"),
            pdf_page_js(3, 7),
            pdf_search_js("plain", 8),
            fill_form_js(&[(4, "plain".into()), (5, "x".into())]),
            network_idle_js(9),
        ] {
            let depth = js.chars().fold(0i32, |d, c| match c {
                '{' | '(' | '[' => d + 1,
                '}' | ')' | ']' => d - 1,
                _ => d,
            });
            assert_eq!(depth, 0, "unbalanced: {js}");
        }
        // Hostile text only ever appears JSON-escaped (its quote can't close the JS string).
        let evil = "a\"b');alert(1)//";
        for js in [
            find_in_page_js(evil),
            pdf_search_js(evil, 8),
            fill_form_js(&[(4, evil.to_string())]),
        ] {
            assert!(!js.contains("a\"b'"), "raw user text leaked: {js}");
            assert!(js.contains(r#""a\"b');alert(1)//""#));
        }
    }

    #[test]
    fn new_tool_actions_parse() {
        use super::super::agent::{AgentAction, parse_action_value};
        let p = |j: &str| parse_action_value(&serde_json::from_str(j).unwrap());
        assert_eq!(
            p(r#"{"action":"find_in_page","text":"가격"}"#),
            Some(AgentAction::FindInPage { text: "가격".into() })
        );
        assert_eq!(
            p(r#"{"action":"read_tab","tab":2}"#),
            Some(AgentAction::ReadTab { tab: 2 })
        );
        assert_eq!(
            p(r#"{"action":"pdf_page","page":0}"#),
            Some(AgentAction::PdfPage { page: 1 })
        );
        assert_eq!(
            p(r#"{"action":"pdf_search","query":"AI"}"#),
            Some(AgentAction::PdfSearch { text: "AI".into() })
        );
        assert_eq!(
            p(
                r#"{"action":"fill_form","fields":[{"target":3,"value":"홍길동"},{"id":4,"value":true},{"value":"no target"}]}"#
            ),
            Some(AgentAction::FillForm {
                fields: vec![(3, "홍길동".into()), (4, "true".into())]
            })
        );
        assert_eq!(p(r#"{"action":"fill_form","fields":[]}"#), None);
        assert_eq!(
            p(r#"{"action":"cite"}"#),
            Some(AgentAction::Cite { style: "apa".into() })
        );
        assert_eq!(
            p(r#"{"action":"wait_for_network_idle","timeout_ms":99999}"#),
            Some(AgentAction::WaitForNetworkIdle { timeout_ms: 15_000 })
        );
        assert_eq!(p(r#"{"action":"screenshot"}"#), Some(AgentAction::Screenshot));
        assert_eq!(
            p(r#"{"action":"bookmarks_search","query":"rust"}"#),
            Some(AgentAction::BookmarksSearch { query: "rust".into() })
        );
        assert_eq!(
            p(r#"{"action":"history_search","query":"pdf"}"#),
            Some(AgentAction::HistorySearch { query: "pdf".into() })
        );
    }

    #[test]
    fn base64_round_trips() {
        for bytes in [&b""[..], b"f", b"fo", b"foo", &[0u8, 255, 16, 137, 80, 78, 71]] {
            let enc = crate::screenshot::base64(bytes);
            assert_eq!(super::super::decode_base64(&enc).as_deref(), Some(bytes));
        }
    }
}
