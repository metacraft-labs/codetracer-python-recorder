# General Issues

## ISSUE-001
### Description
We need to record function arguments when calling a function

We have a function `encode_value` which is used to convert Python objects to value records. We need to use this function to encode the function arguments. To do that we should modify the `on_py_start` hook to load the current frame and to read the function arguments from it.

### Status
Not started


# Issues Breaking Declared Relations

This document lists concrete mismatches that cause the relations in `relations.md` to fail.

It should be structured like so:
```md
## REL-001
### ISSUE-001-001
#### Description
Blah blah blah
#### Proposed solution
Blah blah bleh

### ISSUE-001-002
...

## REL-002
...
```
