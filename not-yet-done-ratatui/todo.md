# TODO

## 1. Misc

- [x] ctrl+j/ctrl+k for down/up in every list and tree (default: arrow keys,
      configurable)
- [x] 2 characters of spacing between the table columns in the tree and the
      list
- [x] Tracking on "A new subitem" flickers back and forth between the two
- [x] Priority column between status and tracking by default
- [x] Extend multi-choice with an ordering function (Ctrl+Up/Down, show_order
      with padding)
- [x] Column selection and ordering settable via multi-choice (stored as a
      setting) (the c key, a popup with checkbox + ordering, persisted in the
      database)
- [x] delete is not a form but a binding that deletes the current element (for
      a node you have to confirm with 'yes'), plus undelete with u
- [x] The highlighting of add, edit and edit node does not go away immediately
      when the editor is closed (sync_components before every draw)
- [x] Tidy up the bars / keybindings. The top should only carry elements where
      activating them shows something in the bar:
      - fuzzy filter
      - search
      - add
      - edit
      - edit node
      - track => highlight it when at least one tracking is running
- [x] When the elements no longer fit into the bar because the terminal is too
      small, there should be an automatic line break. The component should know
      how many lines it needs and the parent should ask for that
      (required_height + a dynamic layout)
- [x] Bug: when you edit an item in edit mode the tree is updated correctly,
      but the cursor position is not (pending_focus_id after the reload)

## 2. Tracking view

- [x] A list of all trackings, with search and fuzzy filter functions like the
      tasks have
- [ ] A summary option similar to timewarrior
- [ ] Opening trackings and the summary in edit mode, similar to the tree view,
      allowing trackings to be edited and moved
- [ ] Saving trackings and the summary
- [ ] Plugins that allow operations to be run and custom summaries to be built

## 3. Post tracking view

- [ ] The shortcut t (configurable) shows every tracking for a node and its
      children in the tree, or for a task in the list

## 4. Minor

- [x] Saving in edit updates the list immediately (live reload on :w)
- [x] Saving in add updates the list immediately (CreateTask → EditTask
      conversion after the first :w)
