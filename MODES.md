# Modes

I want to support multiple modes. Each mode radically affects how you can interact with the underlying file structure and its elements. Here are the modes that I'm thinking about:

1. commander
1. dj
1. director
1. debugger
1. artist

To switch modes, press `esc :` and type the hotkey for the mode.

For example `esc :c` turns on commander mode

## The way that modes work

When you switch to a mode, several hot key mappings become available (depending on the mode). Pressing  `ctrl-shift-k` will show you the mappings globally. `ctrl-shift-m` will show you all the possible modes.

## Commander

In command mode, any time we cd into a directory, we see a list of files above the prompt in a scrollable view. Several hot key combinations show up that allow you to change the view which changes how the listing looks.

For example: `t` would toggle through the kinds of views that you have.

Some views have additional options. Here are the view layouts:

- list
- grid
- carousel

### List

```
+----------------------------------------------+
| | Attributes | Name        | Last Modified | |
| +------------+-------------+---------------+ |
| | drwxr-xr-x | .           | Oct 31 02:02  | |
| | drwxr-xr-x | ..          | Oct 31 02:02  | |
| | drwxr-xr-x | example.txt | Oct 31 02:02  | |
+----------------------------------------------+
```