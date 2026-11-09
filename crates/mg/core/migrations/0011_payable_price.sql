-- A mint-v2 payer funds a quote only in notes of 128 msat and up
-- (locked_payments::denominations::economical_mint_v2_denominations), so a
-- stored price off that grid could never be paid from a mint-v2 federation.
-- Round it up to the next multiple of 128 msat, the value every setter now
-- stores (locked_payments::denominations::payable_price_msats). A changed
-- offer draws a fresh epoch, as every price change does.
UPDATE offer_state
SET price_msats = (price_msats / 128 + 1) * 128,
    offer_epoch = randomblob(32)
WHERE price_msats IS NOT NULL AND price_msats % 128 != 0;
