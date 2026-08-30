# Legacy scripts

The original ad-hoc scripts, kept runnable so their output can be diffed against
the `stk` CLI during migration. They are excluded from lint, type-check and
coverage.

**Delete this directory once you have cross-checked each command below against
your real library.** Nothing is lost by doing so: the scripts are permanently
retrievable from the `original-scripts` tag.

```bash
git rm -r legacy                          # when you are done comparing
git show original-scripts:apply_genres.py # read one afterwards
git checkout original-scripts -- .        # restore the whole original tree
```

Keeping dead code in the tree is worse than deleting it, because the next person
to read the repository cannot tell what is live.

| Script | Replacement |
| --- | --- |
| `dedup_library.py` | `stk wantlist diff` |
| `find_duplicates.py` | `stk dupes titles` |
| `find_duplicates_by_content.py` | `stk dupes content` |
| `check_fake_flac.py` | `stk verify flac` |
| `check_missing_art.py`, `check_art_quality.py` | `stk art check` |
| `embed_missing_art.py`, `fix_art.py`, `audit_art_matches.py` | `stk art fetch`, `stk art fix` |
| `check_genres.py` | `stk genre check` |
| `apply_genres.py` | `stk genre apply` |
| `audit_all_tags.py` | `stk tags dump` |
| `nicotine-plugin/` | `stk fetch search`, `stk review`, `stk fetch download` |
