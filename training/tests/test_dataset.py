import math

from conftest import write_corpus

from spider_optimize_train import cli
from spider_optimize_train import schema as sch
from spider_optimize_train.dataset import Manifest, pairs_of, validate, validate_rows


def names(rows, manifest, needle):
    found = [v for v in validate_rows(rows, manifest) if needle in v]
    return found


def test_synth_corpus_is_valid(corpus_dir, manifest, planted):
    assert validate(corpus_dir) == []
    assert manifest.synthetic is True
    assert manifest.credits_spent == 0.0
    assert manifest.rows == 2 * manifest.pairs
    assert planted["days"] == manifest.day_max - manifest.day_min + 1


def test_too_few_rows_and_pairs_are_named(valid_rows, small_manifest):
    rows = valid_rows[:120]
    problems = validate_rows(rows, small_manifest)
    assert any("too few rows: 120 < 200" in p for p in problems)
    assert any("too few pairs: 60 < 100" in p for p in problems)


def test_a_pair_without_exactly_one_baseline_is_named(valid_rows, small_manifest):
    first = valid_rows[0]["pair"]
    valid_rows[1]["arm"] = "baseline"
    second = valid_rows[2]["pair"]
    valid_rows[2]["arm"] = "shadow"
    problems = validate_rows(valid_rows, small_manifest)
    assert f"pair {first}: 2 baseline arms, expected exactly one" in problems
    assert f"pair {second}: 0 baseline arms, expected exactly one" in problems


def test_arms_on_different_days_are_named(valid_rows, small_manifest):
    pair = valid_rows[1]["pair"]
    valid_rows[1]["day"] += 1
    assert f"pair {pair}: arms on different day" in validate_rows(valid_rows, small_manifest)


def test_arms_on_different_sites_are_named(valid_rows, small_manifest):
    pair = valid_rows[1]["pair"]
    valid_rows[1]["dk"] += 1
    assert f"pair {pair}: arms on different dk" in validate_rows(valid_rows, small_manifest)


def test_an_edit_code_with_too_few_rows_is_named(valid_rows, small_manifest):
    schema = sch.load()
    key = schema.key_index("block_analytics")
    kept, dropped = [], set()
    seen = 0
    for pair, arms in pairs_of(valid_rows).items():
        cand = [a for a in arms if a["arm"] == "candidate"]
        if cand and cand[0]["edit"] and cand[0]["edit"]["key"] == key:
            seen += 1
            if seen > 19:
                dropped.add(pair)
                continue
        kept.extend(arms)
    code = schema.edit_code(key)
    assert names(kept, small_manifest, f"edit code {code} (block_analytics): 19 candidate rows")


def test_an_edit_code_with_one_outcome_is_named(valid_rows, small_manifest):
    key = sch.load().key_index("full_resources")
    for row in valid_rows:
        if row["edit"] and row["edit"]["key"] == key:
            row["success"] = True
    assert names(valid_rows, small_manifest, "(full_resources): only one success outcome")


def test_a_host_like_string_is_named(valid_rows, small_manifest):
    valid_rows[4]["routed"]["source"] = "cdn-beta.example"
    valid_rows[6]["status"] = "https://x"
    problems = validate_rows(valid_rows, small_manifest)
    assert any("row 5" in p and "host-like string at row.routed.source" in p for p in problems)
    assert any("row 7" in p and "host-like string at row.status" in p for p in problems)


def test_a_label_without_a_dot_is_not_host_like(valid_rows, small_manifest):
    valid_rows[4]["status"] = "needs_login"
    assert validate_rows(valid_rows, small_manifest) == []


def test_features_outside_unit_range_or_non_finite_are_named(valid_rows, small_manifest):
    valid_rows[0]["base"][3] = 1.5
    valid_rows[1]["edit_feats"][2] = math.nan
    problems = validate_rows(valid_rows, small_manifest)
    assert any("row 1" in p and "base[3] is 1.5" in p for p in problems)
    assert any("row 2" in p and "edit_feats[2] is nan" in p for p in problems)


def test_feature_lengths_are_named(valid_rows, small_manifest):
    valid_rows[0]["base"] = valid_rows[0]["base"][:151]
    valid_rows[1]["edit_feats"].append(0)
    problems = validate_rows(valid_rows, small_manifest)
    assert any("row 1" in p and "base has length 151, expected 152" in p for p in problems)
    assert any("row 2" in p and "edit_feats has length 97, expected 96" in p for p in problems)


def test_versions_differing_from_the_manifest_are_named(valid_rows, small_manifest):
    valid_rows[0]["edit_feat_v"] = 2
    problems = validate_rows(valid_rows, small_manifest)
    assert any("version edit_feat_v 2 differs from manifest 1" in p for p in problems)


def test_the_cli_exits_one_on_any_violation(tmp_path, valid_rows, small_manifest, capsys):
    good = write_corpus(tmp_path / "good", valid_rows, small_manifest)
    assert cli.main(["validate", str(good)]) == 0
    valid_rows[1]["dk"] += 1
    bad = write_corpus(tmp_path / "bad", valid_rows, small_manifest)
    assert cli.main(["validate", str(bad)]) == 1
    assert "arms on different dk" in capsys.readouterr().out


def test_manifest_round_trips(tmp_path, small_manifest):
    small_manifest.write(tmp_path)
    assert Manifest.read(tmp_path) == small_manifest
