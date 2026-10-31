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

When you switch to a mode, several hot key mappings become available (depending on the mode). Pressing  `ctrl-shift-k` will show you the mappings globally. `ctrl-shift-m` will show you all the possible modes. All key mappings start with a modifier (or mod) key (default: `alt`).

# Phase 1

## Commander

In command mode, any time we cd into a directory, we see a list of files above the prompt in a scrollable view. Several hot key combinations show up that allow you to change the view which changes how the listing looks.

For example: `t` would toggle through the kinds of views that you have.

Some views have additional options. Here are the view layouts:

- list
- grid
- carousel

### List

With the list view, you get a list as you would expect in `ls` with the `lah` options (default). With a little extra.

It shows up in a view above the prompt

```
+----------------------------------------------+
| <pwd>                   options | man | help |
| <search>                                     |
+----------------------------------------------+
| | Attributes | Name        | Last Modified | |
| +------------+-------------+---------------+ |
| | drwxr-xr-x | .           | Oct 31 02:02  | |
| | drwxr-xr-x | ..          | Oct 31 02:02  | |
| | drwxr-xr-x | example.txt | Oct 31 02:02  | |
+----------------------------------------------+
```

We can adjust the flags with the flag option as a hotkey.

### Grid

With the grid view, we get a grid of items in the directory.

It shows in a view above the prompt

```
+----------------------------------------------+
| <pwd>                   options | man | help |
| <search>                                     |
+----------------------------------------------+
| +------+ +------+ +------+ +------+ +------+ |
| |      | |      | |      | |      | |      | |
| |      | |      | |      | |      | |      | |
| +------+ +------+ +------+ +------+ +------+ |
|  p.png   foo.txt    ...      ...      ...    |
| +------+ +------+ +------+ +------+ +------+ |
| |      | |      | |      | |      | |      | |
| |      | |      | |      | |      | |      | |
| +------+ +------+ +------+ +------+ +------+ |
|   ...      ...      ...      ...      ...    |
+----------------------------------------------+
```

You can adjust the grid items to be large or smaller: this changes the wrap on the grid.

Options are made available through the hotkey mapping or the options window.

Each grid item will show a preview of it by type if available. If you click on it (or use the hotkey), it will open a full width view of the file where supported.

If it's video or music related, a player will appear.

## Carousel

Carousel allows you to view items in the folder in a carousel view.

It shows in a view above the prompt
```
+----------------------------------------------+
| <pwd>                   options | man | help |
| <search>                                     |
+----------------------------------------------+
|   +--------------------------------------+   |
| +-|                                      |-+ |
| | |                                      | | |
| | |                                      | | |
| | |                                      | | |
| | |                                      | | |
| | |                                      | | |
| | |                                      | | |
| | |                                      | | |
| | |                                      | | |
| | |                                      | | |
| | |                                      | | |
| | |                                      | | |
| +-|                                      |-+ |
|   +--------------------------------------+   |
|        Filename | Last Modified At           |
+----------------------------------------------+
```

Options are made available through the hotkey mapping or the options window.

Each item will show a preview of it by type if available. If you click on it (or use the hotkey), it will open a full width view of the file where supported.

If it's video or music related, a player will appear.

# Phase 2

## Dj

TBD

## Director

TBD

## Debugger

TBD

## Artist

TBD
