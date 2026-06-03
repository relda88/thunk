## TUI Keybindings
| Key | Behavior |
| --- | --- |
| `Ctrl+C`, `Ctrl+Q` | Quit |
| `Enter` | Submit input, accept launcher, or accept reverse search depending on active mode |
| `Alt+Enter` | Insert newline |
| `Backspace` | Delete before cursor, launcher query char, or reverse-search query char depending on active mode |
| `Alt+Backspace`, `Ctrl+W` | Delete word before cursor |
| `Left`, `Right` | Move cursor |
| `Home`, `End` | Move to current logical line start/end |
| `Ctrl+D` | Dump last assembled prompt to temp file |
| `Ctrl+P` | Recall previous input |
| `Ctrl+N` | Reject pending approval, otherwise recall next input |
| `Ctrl+Y` | Approve pending approval |
| `Up`, `Down` | Cycle launcher selection when launcher is active; otherwise scroll transcript by 1 |
| `PageUp`, `PageDown` | Scroll transcript by 10 |
| `Ctrl+O` | Toggle expanded file-read transcript view |
| `Ctrl+K` | Open command launcher when not busy |
| `Ctrl+R` | Start/cycle reverse search |
| `Esc` | Cancel launcher, autocomplete, or reverse search depending on active mode |
| `Tab` | Forward slash-command autocomplete when not busy |
| `Shift+Tab` / `BackTab` | Reverse slash-command autocomplete when not busy |
| `Alt+[` | Focus previous collapsible block where supported by terminal protocol |
| `Alt+]` | Focus next collapsible block |
| `Alt+O` | Toggle focused collapsible block |
| Printable characters | Insert into input, launcher query, or reverse-search query depending on active mode |