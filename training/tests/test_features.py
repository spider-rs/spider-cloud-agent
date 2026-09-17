import numpy as np
import pytest

from spider_optimize_train import features as feat
from spider_optimize_train import schema as sch
from spider_optimize_train import synth


def test_pinned_bucket_edges():
    cases = {0: 0, 1: 1, 2: 1, 3: 2, 5: 2, 6: 3, 40: 3}
    for count, bucket in cases.items():
        low = (1 << min(count, 32)) - 1
        high = (1 << max(count - 32, 0)) - 1
        assert feat.pinned_bucket(low, high) == bucket, count


def test_the_matrix_is_base_then_edit_then_one_pinned_bucket(corpus_rows):
    rows = corpus_rows[:400]
    arrays = feat.build(rows)
    edit_dim = sch.load().edit_dim
    assert arrays.X.shape == (400, 152 + edit_dim + 4)
    assert np.all(np.isfinite(arrays.X))
    for i, row in enumerate(rows):
        assert np.array_equal(arrays.X[i, :152], np.asarray(row["base"], dtype=np.float32))
        pins = arrays.X[i, 152 + edit_dim :]
        assert pins.sum() == 1.0
        assert pins[feat.pinned_bucket(row["pinned"], row["pinned_hi"])] == 1.0


def test_edit_codes_are_compact_and_follow_schema_order():
    schema = sch.load()
    learnable = [k["index"] for k in schema.keys if k["learnable"]]
    codes = [schema.edit_code(i) for i in learnable]
    assert codes == list(range(1, len(learnable) + 1))
    assert schema.edit_code(None) == sch.KEEP_CODE == 0
    assert schema.edit_code(schema.key_index("url")) is None
    assert schema.edit_codes == len(learnable) + 1
    assert sch.cell_id(5, 13, 3, 9) == (5 << 24) | (13 << 16) | (3 << 8) | 9
    with pytest.raises(ValueError):
        sch.cell_id(256, 0, 0, 0)


def test_synth_writes_the_edit_blocks_where_the_schema_puts_them():
    schema = sch.load()
    site = synth.make_sites(np.random.default_rng(0), 1)[0]
    wait = synth.standard_edit("wait_for")
    x = synth.edit_features(site, wait)
    learnable = [k["wire"] for k in schema.learnable]
    assert x[schema.block("edit_key") + learnable.index("wait_for")] == 1
    assert x[schema.block("edit_op")] == 1
    assert x[schema.block("value_bucket") + sch.WAIT_BUCKETS.index(5000)] == 1
    assert x[schema.block("bias")] == 1
    assert len(x) == schema.edit_dim
    keep = synth.edit_features(site, None)
    lo, hi = schema.block("edit_key"), schema.block("need_bits")
    assert sum(keep[lo:hi]) == 0
    assert keep[schema.block("missing") + 1] == 1
