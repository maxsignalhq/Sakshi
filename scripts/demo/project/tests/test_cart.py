from src.cart import apply_coupon, total


def test_total_sums_line_items():
    assert total([(10.0, 2), (5.0, 1)]) == 25.0


def test_discount_is_applied():
    assert total([(100.0, 1)], discount=0.2) == 80.0


def test_coupon_never_goes_negative():
    assert apply_coupon(5.0, "SAVE10") == 0.0
    assert apply_coupon(50.0, "SAVE10") == 40.0
