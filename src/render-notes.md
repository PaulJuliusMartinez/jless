# Notes:

- Want to enable "scroll off", which means selection will
  never get with N lines of the top or bottom of the screen.
- When jumping to an element that's above the top scroll-off,
  that element should then be focused at line N.
- When hitting down arrow repeatedly, or jumping past the bottom,
  the element should appear at line H - N.

- vim sometimes puts the newly focused element in the middle, but
  it's not exactly clear when it does this; perhaps if it's a "big
  enough" jump? (How big is enough?)

- When scrolling with C-e/C-y, focus automatically moves as well
  when it gets pushed out of scroll-off zone

- If implementing something like "collapse me and all my siblings",
  then the lines above the currently focused element could change.

## Line Wrapping:
- Wow, line wrapping makes this all super hard
- Can detect string widths using `unicode_width`
  - How accurate is it?
  - For emojis and other things, probably _over_estimates width, which
    is good.
  - If there's a bug with some crazy characters that's fine
  - If just goes one extra line, then just means that scroll off might
    be off in places (it'll just push top of the screen off)
      - Maybe impossible to see the top of the screen (if implementation
        is buggy)
  - What about SUPER long strings that fill entire screen.
    - I don't care
- Really only makes optimizations that redraw only parts of the screen more difficult
- INITIALLY, won't support wrapping, and will automatically condense
  long strings / inlined objects
  - Will assume that we won't have super indented objects, or super long
    object keys.

## How to implement all of this?

### First step:
- Support rendering from a certain point, but fill the screen.
- This needs to support starting at a closing brace
- This sort of reference to a starting point is different than a
  focus, but maybe just (focus, start_or_end (_or_primitive ?))
- Similar issue with container state (expanded/inlined/collapsed),
  where we only want that state to apply to containers; here we only
  want start/end to apply to containers, and not primitives.
- Probably just use a simple bool / 2/3-state enum.
- Maybe primitives also use start? an inlined / collapsed container
  is basically a primitive.

### Next step:
- When changing focus, figure out what line the focused element will
  be on, by starting at "start" point and walking the tree forward until
  we find focused element.
- Walking this structure is 'easy' (linear), only need to walk # of lines
  in screen, before giving up.
- If focused element isn't on screen, we can work backwards from where we
  want it to be to get "start" location of screen

### Algorithm:



## Optimizations:
- It'd be great if we didn't have to print the whole screen each time.
  - Actually would it be "great", is printing whole screen each time slow?
