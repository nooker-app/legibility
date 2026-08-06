<!--
ADR 0001 — PROBE-B
Status: ACCEPTED
Date: 2026-08-06
Gate: M0 exit criterion. M1 does not start without this ADR.
Method: 5 parallel source-grounded assessments of dom_smoothie 0.18.0 (src 4,297 LOC) plus
        dom_query 0.28.0 (the substrate), then two adversarial cases (contribute vs build),
        then this synthesis. Every claim carries a file:line citation. No files were modified.
-->

# PROBE-B ADR — dom_smoothie 0.18.0 vs legibility

**BUILD — independent implementation as planned, dom_smoothie used as prior art and as a baseline**

Kill criterion은 conjunction이다: "결함 1/2/3의 대부분을 닫고 있고" **AND** "구조가 확장 가능하다면". 앞쪽 절이 측정으로 먼저 죽는다 — 결함 3은 0%다. `Article`은 14개 flat public field에 `content: StrTendril` 하나와 `text_content` 하나뿐이고 `#[non_exhaustive]`도 없으며(`src/readability.rs:19-65`, 전문 확인), `CONTENT_ID = "readability-page-1"`(`src/glob.rs:11`)이 "region은 정확히 하나"를 파이프라인 전체에 못박아 놨다. 결함 1은 ~8%: score가 `data-readability-score` DOM attribute에 십진 문자열로 직렬화되어 매 read마다 재파싱되고(`src/score.rs:7-19`, `src/glob.rs:13`), attribute 존재 여부가 "이 노드는 scoring에서 방문됐다"는 set predicate로 겸용되며, page-relative statistic이 crate 전체에 하나도 없다. retry ladder는 `char_threshold` 절대값 기준으로 Document를 pass마다 deep clone하고(`src/grab.rs:28`, `:37-40`), 다 실패하면 `best_attempt` — 가장 긴 텍스트 — 를 반환한다(`:50-61`, 전문 확인). 그리고 charter가 code보다 단단하다: `README.md:12`은 "closely follows the implementation of readability.js"이고, `CHANGELOG.md:19-21`(0.17.0)은 4-column TextRole이 요구하는 two-stage pre/post-scoring split을 **의도적으로 삭제**했으며, golden suite는 mozilla `expected.html`에 대한 whitespace-strip 후 **정확한 HTML 동등성**이다(`tests/common.rs:132-146`, 전문 확인). 결함 1 수정은 정의상 이 suite를 전부 깬다.

두 assessment가 "이 2시간 확인이 890h 커밋보다 먼저 와야 한다"고 지목한 미해결 전제를 내가 직접 읽었고, **양쪽 케이스가 각자의 핵심 논거에서 틀렸다**. 아래 두 정정이 이 ADR의 고유 기여다.

## 정정 1 — Send는 구조적으로 배제되어 있지 않다 (CASE 2의 논거 4 반증)

dom_query 0.28에는 **`atomic` feature가 존재한다**(`dom_query-0.28.0/Cargo.toml:50`). 켜면 `pub type StrWrap = Tendril<fmt::UTF8, tendril::Atomic>`이고 `wrap_tendril`은 `v.into_send().into()`로 변환한다(`dom_query-0.28.0/src/entities.rs:15-22, 38-42`). 그리고 `dom_query-0.28.0/examples/send_document.rs`는 `thread::spawn` + channel로 `Document`를 **실제로 스레드 경계 넘겨 보낸다**(`Cargo.toml:88` `required-features = ["atomic"]`). dom_smoothie는 그냥 이 feature를 안 켰을 뿐이다(`Cargo.toml:97-100`: `mini_selector`, `markdown`만). 즉 "NonAtomic이 markup5ever를 통해 hard-wire되어 있어 7개 foundation crate를 교체해야 한다"는 서술은 **틀렸다** — feature passthrough + `into_send()`로 끝나는 additive 변경이다. 이 항목은 plan의 난이도 표에서 내려야 한다.

## 정정 2 — side table은 가능하다, dense column은 불가능하다 (CASE 2의 논거 1을 정밀화, CASE 1의 flip 조건 해소)

`Tree { pub(crate) nodes: RefCell<Vec<TreeNode>> }`(`dom_query-0.28.0/src/dom_tree/tree.rs:20-22`), `TreeNode`는 `data: NodeData`를 품은 AoS(`src/node/inner.rs:8-23`), `NodeId { pub(crate) value: usize }`이지만 `derive(Copy, Eq, Hash, Ord, PartialOrd)`(`src/node.rs:24-27`). 결론은 정확히 이렇다 — **`HashMap`/`BTreeMap<NodeId, T>` 외부 side table은 가능**하고 dom_smoothie가 이미 그렇게 쓰고 있다(`src/grab.rs:235`). **dense `Vec` 인덱스 SoA column은 불가능**하다: `value`와 `nodes`가 둘 다 `pub(crate)`다. 그리고 `NodeId: Ord`가 생성 순서 = 파싱 문서 순서이므로 **document-order tie-break은 오늘 additive하게 가능하다**. 따라서 CASE 1의 "flip 조건"(side table 불가면 dom_query까지 fork)은 발동하지 않고, CASE 2의 논거 1은 강한 형태(parse-time hook 없음, dense column 없음)로는 살아남지만 서술된 형태("붙일 column이 없다")로는 과하다.

이 두 정정은 **CONTRIBUTE로 기울이지 않는다.** 오히려 CASE 1의 유일한 decisive 논거를 무너뜨린다: `test-pages`는 published crate에 없고(`Cargo.toml:19-23` exclude, `ls test-pages` → 부재, 확인), 그 corpus는 `source.html` + `expected.html` + `expected-metadata.json` 구조 그대로 **mozilla/readability의 것**이다. "greenfield가 쓸 수 없는 가장 비싼 자산"은 dom_smoothie의 자산이 아니라 upstream 자산이고, mozilla/readability에서 직접 받으면 된다. plan은 이미 `tools/rjs-baseline`(committed lockfile)을 갖고 있다. fork가 corpus로 얻는 것은 ~0이다.

## 요구사항 클러스터별 판정

| 클러스터 | 판정 | 근거 (한 줄) |
|---|---|---|
| **결함 1 (length)** | architecture precludes | score가 DOM attribute의 문자열(`score.rs:7-19`), page-relative 통계 0개, ladder가 품질 메커니즘 자체(`grab.rs:37-40, 50-61`) — ~8% |
| **결함 2 (metadata)** | **additive PR** | 전부 `readability.rs:560-853` + `:1013-1015` + `:1103-1186`에 국소화, precedence는 `if metadata.X.is_none()` chain으로 이미 존재; verbatim은 `Readability`(`:113-120`)에 `source: StrTendril` 1개 추가로 containment 형태로 집행 가능 — 입력 tendril은 `Document::from`에 버려지고 있을 뿐(`:122-130`) |
| **결함 3 (comments)** | architecture precludes | `Article` 14 flat field·region kind 없음(`readability.rs:19-65`), `CONTENT_ID` 싱글턴(`glob.rs:11`), mask를 넣으면 HN에서 4 pass 전부 `char_threshold` 실패 후 "댓글을 가장 많이 흘린 pass"가 반환됨(`grab.rs:37-40, 56-60`) — 0%, 결함 1과 용접됨 |
| **결함 4 (perf/arch)** | architecture precludes | feature store가 DOM 그 자체; div마다 subtree 재순회 ~15회(`prep_article.rs:51-184`), pass마다 Document deep clone(`grab.rs:28`) + `best_attempt` 5번째 사본 — ~15% |
| **a11y / TextRole 4-column** | architecture precludes (one-liner만 additive) | control 텍스트가 `prep_article.rs:372`(=`glob.rs:53-54`)에서 제거되는데 scoring은 `grab.rs:87`에서 이미 끝났다 — 정확히 그 ordering 버그; column은 parse 중에 채워야 하고 dom_query는 parse hook도 dense column도 노출하지 않음 |
| **stability S1-S5** | 대부분 precludes, 개별 수정은 additive | panic 개별건은 (b)이고 재현했다(아래) — 그러나 never-panic *보장*은 dom_query/html5ever recursion을 소유하지 않아 불가; determinism은 tie-break만 (b)(`NodeId: Ord`), 보장은 foldhash 순서(`grab.rs:235, 299`) + f32→String 왕복(`score.rs:7-19`)으로 불가; parity gate는 exact-equality golden이 프로젝트 정체성이라 (c); sanitizer 2-profile은 sanitizer도 없고 출력도 하나라 (c) |
| **portability / iOS** | Send는 **additive**, no_std·spans는 precludes | Send: dom_query `atomic` feature로 해결됨(정정 1); no_std: src에 `#![no_std]`/`extern crate alloc` 0건이고 foundation 7개 crate에 no_std 모드 없음; byte span: `TreeSink`의 유일한 위치 hook이 `set_current_line(u64)`뿐 |
| **CJK** | **additive PR** | `COMMAS`에 U+3001 없음(`glob.rs:167-170`, 확인 — U+FE11 세로형은 있는데 원형이 없다), `is_sentence`는 ASCII period 전용(`matching.rs:66-68`, 확인), NFC 정규화 0건 |
| **explainability** | architecture precludes | `Article`에 confidence/reason/trace 없고 score는 출력 전에 의도적으로 제거됨(`readability.rs:879-882`); 채울 값이 없다 — unnormalized 절대 점수는 confidence가 아니므로 결함 1 수정에 의존 |

CJK panic은 직접 재현했다. `matching.rs:53-64`의 `pos > 1` guard는 char boundary가 아니라 byte 거리를 검사한다. 함수를 그대로 떼어 `rustc -O`로 빌드·실행: `is_video_url("一youtube.com/x")` → `byte index 1 is not a char boundary; it is inside '一' (bytes 0..3)`. `prep_article.rs:22/25/92/96`에서 모든 `object`/`embed`/`iframe`의 모든 attribute와 `inner_html()`에 대해 호출되므로 `<object title="一youtube.com">` 하나로 파싱 전체가 abort된다. remote DoS이고 2줄 수정이다.

## 계획에 반영할 변경

**(1) dom_smoothie가 M1의 175h `legibility-legacy` port를 conformance oracle로 대체할 수 있는가 — 아니다.**
두 가지 독립적 이유로 안 된다. 첫째, fidelity가 하필 legibility의 표적 구간에서 깨진다: `min_score_to_adjust` 기본 5.0(`config.rs:49, 73`)이라 점수 ≤5.0인 candidate는 link-density 정규화를 **아예 건너뛴다**(`grab.rs:289-293`, 확인). R.js는 무조건 `*= (1 - linkDensity)`를 적용한다. 즉 짧은 글·링크 글(2 + commas + len/100 → 2..5 구간)에서 dom_smoothie는 R.js가 아니다 — parity gate가 겨냥하는 바로 그 case다. 여기에 en/em dash title over-split(`glob.rs:172`에 '–','—' 포함, R.js는 space-delimited ASCII만), `README.md:358` 이하가 스스로 열거하는 filtering-order divergence(+ 미테스트 `test-pages/not-matching` 보관소), `CHANGELOG 0.17.0`의 single-pass 재작업이 겹친다. 둘째이자 더 결정적으로, port의 존재 목적은 실패를 `INFRA`/`PARSER`/`ARITH`로 분해하는 것이다(plan line 674). `INFRA` 판정은 **legibility 자신의 arena와 parser에 대한 주장**이다. dom_smoothie는 dom_query/html5ever 위에서 돌기 때문에 그 판정을 원리적으로 생산할 수 없고, 대체하면 남은 700h 내내 "내 arena 문제인가 내 scorer 문제인가"가 되돌아온다. 게다가 R.js 권위는 이미 `tools/rjs-baseline`이 갖고 있으므로 대체가 사는 것도 아니다.
**대신 확보되는 절감:** dom_smoothie는 MIT(`Cargo.toml:40`, `LICENSE` Copyright 2024 Mykola Humanov)이고 R.js 산술이 Rust idiom으로 이미 전사되어 있다 — `score.rs`(106줄), `glob.rs`의 word list·COMMAS·title separator·meta key table, `url_helpers.rs`(207), `matching.rs`(337), JSON-LD field map. port를 **없애지 않고** 전사 시간만 깎는다: M1/M3에서 20-35h, 아키텍처 결합 0. 175h를 유지하되 이 절감을 반영해 ~145-155h로 재산정하라.

**(2) parity harness의 두 번째 differential baseline으로 추가할 것인가 — 예. 조건부로.**
~10-15h. in-process, Node 불필요, fuzz loop 안에서 돌 만큼 빠르고, 자체 reachable panic들이 무료 crash-differential 신호를 준다. 무엇보다 Rust-vs-Rust 비교라 arena/parser 차이와 scoring 차이를 분리해 준다 — R.js+jsdom 단일 baseline이 못 하는 일이다. 조건 네 개: 권위가 아니라 관측점으로만 쓴다; 위에 열거한 divergence(`min_score_to_adjust`, en/em dash, filtering order)를 **expected-divergence로 등록**해 실패로 세지 않는다; 버전 pin한다; corpus는 dom_smoothie에서 못 가져오므로(published crate에 `test-pages` 없음) mozilla/readability에서 직접 vendor한다.

**그 외 구체적 변경:**
- **Send 항목을 어려운 칸에서 내린다.** dom_query `atomic` feature가 생태계 패턴(`tendril::Atomic` + `into_send`)을 증명했다. 해당 line item 하향 재산정.
- **corpus 항목은 dom_smoothie 때문에 줄이지 않는다.** mozilla/readability에서 직접 받는다 — dom_smoothie의 fixture 출처가 거기다.
- **무료 upstream PR 6건을 지금 보낸다** (주말 1회, 890h와 무관하게): CJK char-boundary panic(`matching.rs:53-64`), U+3001 → `COMMAS`(`glob.rs:169`) + `prep_article.rs:81`이 ASCII comma 대신 `COMMAS` 사용, `is_sentence`에 U+3002/FF01/FF1F(`matching.rs:66-68`), `javascript:` case-sensitive 우회(`glob.rs:41`) + scheme allowlist, `total_cmp` + `(score bits, NodeId)` tie-break(`grab.rs:299`) 및 역전된 comparator(`grab.rs:454-457`), JSON-LD `@graph` 버그 + gjson `"@`→`"^` 값 훼손(`readability.rs:614-627, 572`). 한국어 테스터가 실제로 부딪히는 것을 지금 고치고, 동시에 maintainer 경계를 공짜로 탐침한다.
- **결함 2는 upstream으로 보낸다.** legibility가 자체 구현하더라도, `detail` feature flag 뒤 provenance PR + `Metadata`를 lossy `From` projection으로 유지(`tests/common.rs:45-57` 무변경 통과)는 ~600-900 LOC 단일 파일이고 favicon은 이미 `Vec<(String, f32)>`를 만들어 한 줄 뒤에 버린다(`readability.rs:1135-1180`). 유일하게 CONTRIBUTE가 정답인 클러스터다.
- **M1 착수 전 두 가지를 확정한다:** (a) "byte offset을 caller에게 반환"인가 "fabrication 없음을 증명"인가 — 전자면 parse를 소유해야 하고 후자면 containment로 끝난다. 이 모호성이 900 LOC 모듈과 새 engine의 차이다. (b) maintainer에게 issue 하나: Config-gated scale-invariant mode와 second output region을 받을 의사가 있는가. `CandidateSelectMode`(`config.rs:14-18`)가 default-off opt-in divergence 선례이므로 답이 예면 이 ADR을 재검토해야 한다. 비용은 issue 한 개다.

## 선택한 경로의 정직한 리스크

- **890h는 백지 기준 가격이고, 미계상 항목은 no_std HTML5 tokenizer + tree builder다.** dom_query는 substrate로 실격이다(`nodes`/`NodeId.value`가 `pub(crate)`, no_std 없음, span 없음) — 따라서 legibility는 tree construction을 소유해야 하고, adoption agency나 foster parenting을 틀리면 **모든 heuristic이 틀린 tree 위에서 측정된다**. plan에 이 항목이 신뢰성 있게 >200h로 잡혀 있지 않다면 일정이 틀린 것이고, 잡혀 있다면 heuristic 한 줄 돌기 전에 저녁 시간 1년이 사라진다.
- **솔로 890h는 first ship까지 16-21개월이고, 그 기간 동안 primary tester의 한국어 사이트는 계속 깨져 있다.** dom_smoothie는 오늘 동작하고 a11y ~35%를 닫고 있으며 위 PR 6건이면 주말에 개선된다. "계획이 옳지만 무관해지는" 리스크가 실재한다 — 완화책은 M0-M2 이후 조기 배포 가능한 subset을 먼저 내는 것이고, 이는 plan이 이미 인정한 방향이다.
- **가장 어려운 요구사항들이 self-imposed라 외부 baseline으로 반증되지 않는다.** blake3 cross-target byte-identical determinism, 4-column TextRole, 6-shape taxonomy는 R.js도 dom_smoothie도 측정해 주지 않는다. 자기 gate를 전부 통과하면서 한국어 corpus에서 R.js에 지는 상태가 가능하다. 완화책이 정확히 plan-edit (2) — 두 번째 baseline + absolute floor — 이고, 그래서 그것이 yes다.

## 무엇이 있었다면 CONTRIBUTE였는가 (반증 가능성)

1. **결함 3이 0%가 아니었다면.** `Article`에 region/comments field가 있거나 `CONTENT_ID`가 싱글턴이 아니었다면, mask는 `grab.rs:582-584`의 ~10줄 diff로 끝난다. 지금은 mask를 넣으면 HN이 **더 나빠진다**(`grab.rs:37-40, 56-60`) — 이 용접이 defect 3을 독립 PR로 만들 수 없게 하는 유일한 이유다.
2. **dom_smoothie가 자기 parse를 소유했거나, dom_query가 `nodes`/`NodeId.value`를 pub으로 노출하고 parse-time hook을 주었다면.** 그러면 TextRole 4 column과 dense SoA가 (b)가 되고, 정정 1(Send 해결)과 합쳐 portability까지 넘어온다. 확인 결과 둘 다 `pub(crate)`다 — 이것이 내가 직접 읽어 확정한 부분이며, 반대로 나왔다면 이 ADR은 FORK였다.
3. **charter가 "beat R.js"였다면.** `README.md:12`가 parity를 선언하고, maintainer는 자기가 default로 출하하는 heuristic을 "flawed"라 쓰면서도(`grab.rs:381-391`) 더 나은 자기 아이디어를 opt-in 뒤에 두었고, `CHANGELOG 0.17.0:19-21`은 필요한 seam을 원칙에 따라 3 릴리스 전에 삭제했다. 3 릴리스 전에 제거된 seam을 되돌리라는 PR은 contribution이 아니다.
4. **golden suite가 similarity/ratchet 기반이었다면** re-bless가 가능해 벽이 사라진다. 지금은 exact HTML equality다(`tests/common.rs:132-146`).
5. **상시 반증 기준 (측정이 code reading을 이긴다):** Korean + short-post + HN corpus에서 frozen R.js golden 대비 differential을 돌려 dom_smoothie가 legibility의 표적 case 중 **≥90%를 이미 R.js와 동등하거나 낫게** 처리한다면, 8%/15%/0%는 측정으로 반박된 것이고 이 ADR은 폐기해야 한다. `min_score_to_adjust=5.0`와 `char_threshold` ladder가 반대를 예측하지만, 이 측정은 싸고 plan-edit (2)를 실행하면 **어차피 공짜로 얻어진다** — M1 안에서 실제로 확인하라. 자존심이 아니라 결과가 기준이라면, 이 숫자가 나오는 순간 결정을 뒤집을 준비가 되어 있어야 한다.

읽은 경로: `/private/tmp/claude-501/-Users-tim-Projects-personal-readability-wasm/c2e79cd4-34de-40da-8302-ffedc6240d3d/scratchpad/probe-b/dom_smoothie-0.18.0` (src 4,297 LOC 확인) 및 `/private/tmp/claude-501/-Users-tim-Projects-personal-readability-wasm/c2e79cd4-34de-40da-8302-ffedc6240d3d/scratchpad/dom_query-0.28.0` (양쪽 assessment가 읽지 못했다고 명시한 substrate). 파일 수정 없음.
