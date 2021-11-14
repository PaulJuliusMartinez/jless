# Search

## vim search

- Type /term, hit enter, jumps to it
- Bottom of screen says [M/N] indicating matches, then
  specifies "W [1/N]" when wrapping


- With default settings:
  - Cursor disappears when you hit '/'
  - Matches get highlighted as you type
    - Next match is white
    - Other matches are yellow
  - Matches stay highlighted

- There is a setting (`redrawtime`) that specifies max time
  spent finding matches

- `hlsearch` indicates whether results are highlighted when done searching
- `incsearch` indicates whether results are highlighted while typing term

## Our initial implementation

- No highlighting as you type or after searching
- Need to store last search term
  - Also computed regex
- Need to store list of matches
  - We will find ALL matches at first
  - Store them in a single vector?

- Do we need to store a index of our last jump anywhere?
  - Multiple matches in a single line
  - Store last match jumped to

## Desired behavior

- Incremental + highlight search until a non-search key is pressed
  (either `n`/`N` or `*`/`#`)
  - Need to store whether actively searching still
- Match is highlighted in string values
  - How are multiline searches highlighted?
  - How are keys highlighted?
- Need to handle true search vs. key search slightly differently


### Collapsed Containers

```
{
  a: "apple",
  b: [
    "cherry",
    "date",
  ],
  c: "cherry",
}

{
  a: "apple",
  b: ["cherry", "date"],
  c: "cherry",
}
```

When a match is found in a collapsed container (e.g., searching for
`cherry` above while `b` is highlighted), we will jump to that key/row,
and display a message "There are N matches inside this container", then
next search will continue after the collapsed container.

- Maybe there's a setting to automatically expand containers while
  searching.

## Search State

```
struct SearchState {
  mode: enum { Key, Free },
  direction: enum { Down, Up },
  search_term: String,
  compiled_regex: Regex,
  matches: Vec<Range<usize>>,
  last_jump: usize,
  actively_searching: bool,
}
```

## Search Inputs

`/` Start freeform search
`?` Start reverse freeform search
`*` Start object key search
`#` Start reverse object key search
`n` Go to next match
`N` Go to previous match


When starting search =>
  - Create SearchState with actively searching `false`
    - Need mode & direction naturally
    - Need search term
    - Need json text
  - Then basically do a "go to next match"

## Messaging:

- Show search term in bottom while "actively searching"
- After each jump show `W? [M/N]`
- On collapsed containers display how many matches are inside
- "No matches found"
- Bad regex


## Tricky stuff

- Updating immediate search state appropriately
