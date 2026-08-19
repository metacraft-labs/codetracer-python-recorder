// Negative-routing sample for the launcher <-> recorder compatibility E2E.
//
// This file exists to be REFUSED, not recorded.  `.rs` is deliberately not
// declared by `codetracer/resources/codetracer-desktop-capabilities`, so
//
//     ct record launcher_compat_unhandled.rs
//
// must fail through the launcher's router with
// `ct: no component handles 'record' for '.rs'` and a non-zero exit, rather
// than silently succeeding or falling through to some other component.
//
// It is an unhandled EXTENSION on purpose (design §5.3 step 8).  A dot-less
// argument would exercise a different, already-known routing gap that milestone
// LRC-4 owns, not the "extension the desktop component does not declare" case
// this scenario is about.
//
// Nothing compiles or runs this file; keep it syntactically plausible but
// dependency-free.

fn main() {
    println!("this program is never recorded");
}
