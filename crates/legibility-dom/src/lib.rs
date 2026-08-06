//! Sole html5ever consumer. Owns the SoA `TreeSink`, tag interning and `doc_buf`.
//! M1 fills this in; M0 only establishes that the seam exists and that core stays parser-free.
#![forbid(unsafe_code)]
