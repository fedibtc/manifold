-- 0006 recorded every existing FMan as not ready without drawing a new offer
-- epoch, so a quote issued before the upgrade still matched and could admit a
-- seat while readiness kept failing. An FMan from before readiness existed
-- was selling, so record it as ready: its first failing run then flips the
-- verdict and draws a fresh epoch, refusing those quotes.
UPDATE offer_state SET ready_for_new_seats = 1;
