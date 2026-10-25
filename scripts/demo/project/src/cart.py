def total(items, discount=0.0):
    subtotal = sum(price * qty for price, qty in items)
    return round(subtotal * (1 - discount), 2)


def apply_coupon(total_amount, coupon):
    if coupon == "SAVE10":
        return total_amount - 10
    return total_amount
