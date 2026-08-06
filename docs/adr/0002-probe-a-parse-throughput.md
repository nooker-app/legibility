<!--
ADR 0002 — PROBE-A
Status: ACCEPTED
Date: 2026-08-06
Gate: M0 exit criterion. Every performance figure in the plan derives from the (b) number here.
-->

# PROBE-A — html5ever throughput, measured twice

## 결정

**PASS.** html5ever 0.39.0 + our featurizing sink는 **127.9 MiB/s**를 낸다. kill 임계값(~40 MiB/s)의
**3.2배**이므로 optimization ladder(`<` SIMD pre-scan → tokenizer-only → fast-path tokenizer)를
M6 이전으로 끌어올릴 필요가 없다. 예산은 아래 (b)에서 유도한다.

## 측정값

재현: `cargo run --release -p legibility-dom --example probe-a` (repo 루트, submodule 체크아웃 상태)

| 항목 | 값 |
|---|---|
| corpus | mozilla/readability test-pages **130 docs**, **24.53 MiB**, arena **673,359 nodes** |
| 하드웨어 | Apple Silicon (aarch64-apple-darwin), release + LTO fat + codegen-units 1 |
| 시행 | best of 3 |
| **(a) null sink** | 146.0 ms → **168.0 MiB/s** |
| **(b) full featurization** | 191.8 ms → **127.9 MiB/s** |
| 우리 오버헤드 | 45.8 ms → 파싱 대비 **1.31×** |

측정일 2026-08-06. 위 숫자는 전부 실행 결과이며, 이 문서에 추정치는 없다.

## 왜 두 번 재는가

계획서 M0가 한 숫자가 아니라 두 숫자를 요구한 이유는 **차이가 예산의 유일한 정직한 근거**이기 때문이다.

- **(a) null sink**는 html5ever의 tokenizer + tree builder만 돌리고 결과를 버린다. 이건 **천장**이다 —
  어떤 arena도 자기를 먹여주는 파서보다 빠를 수 없다. 하지만 **사용자가 관측할 수 없는 숫자**다.
- **(b)**는 실제 `BuildArena`가 tag interning, a11y 분류, `doc_buf` 복사를 하고 그 뒤에 `flatten()`과
  `accumulate_subtrees()`까지 끝낸 값이다. **계획서의 모든 성능 서술은 이 숫자에서 나온다.**

(a)를 인용하는 것은 아무도 경험할 수 없는 수를 인용하는 것이다. 그래서 예산 표에는 (b)만 들어간다.

## 읽어낼 것

**오버헤드 1.31×가 이 아키텍처의 논거다.** two-phase arena(mutable doubly-linked build → `flatten()` →
단일 reverse pass) 전체가 파싱 비용의 31%다. 여기에는 a11y 4-way 분류와 4개 length column 누적이
이미 포함되어 있다 — 즉 §1.10의 "통계 오염 방지"가 사실상 무료다. 이걸 나중에 cleaning 단계에서
하려고 했다면 트리를 한 번 더 순회해야 했을 것이다.

**아직 측정하지 않은 것을 분명히 한다.** 이 수치는 Readability.js + jsdom과 비교되지 않았다. 비교는
M1의 `xtask stability-baseline`이 담당하고, 그때까지 "R.js보다 N배 빠르다"는 서술을 쓰지 않는다.
계획서의 대조군 추정치(400–900 ms)는 **여전히 미검증 가설**이다.

## 조건부 후속

`legibility-core`의 scoring·segmentation은 아직 없다(placeholder만 존재). (b)는 M6에서 12 derived
feature와 256-bucket histogram이 들어오면 반드시 나빠진다. 그때 이 example을 다시 돌려 값을 갱신하고,
**목표에 미달하면 삭제하지 않고 측정된 사실로 재진술한다**(계획서 M10 원칙).
