// Fixture: exports, a dead arrow function, and two clones.

export function greet(name) {
  return `hello ${normalize(name)}`;
}

function normalize(value) {
  return value.trim().toLowerCase();
}

const legacyFormatter = (value) => {
  const trimmed = value.trim();
  return trimmed.toUpperCase();
};

export function totalCents(items) {
  let total = 0;
  for (const item of items) {
    if (item.active && item.price > 0) {
      total += Math.round(item.price * 100);
    }
  }
  return total;
}

export function totalEuros(items) {
  let total = 0;
  for (const item of items) {
    if (item.active && item.price > 0) {
      total += Math.round(item.price * 100);
    }
  }
  return total;
}
