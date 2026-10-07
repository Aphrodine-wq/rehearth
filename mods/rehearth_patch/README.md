# ReHearth Patch

Bug and lag fixes for Stonehearth, built on top of ACE (requires ACE 0.9.6+).

## 0.1.0

- **Item search does half the pathfinding.** When a hearthling looks for an item
  "anywhere", the game searches the ground and storage at the same time. Both
  searches used to keep running after one found something, so every find cost a
  second full pathfinding search (and a `duplicate "set_think_output"` warning in
  `stonehearth.log`). Now the first search to find something cancels the other.

Files derived from ACE are MIT licensed by the Stonehearth ACE Team (see
`LICENSE.ace.md`).
