/**
 * Create a comparator function to sort objects by a specific string property alphabetically,
 * ignoring case, with a specific keyword (e.g., "latest") always at the top.
 *
 * @param {string} property - The object property to sort by.
 * @param {string} keyword - The special keyword to keep at the top (case-insensitive).
 * @returns {(a: Record<string, any>, b: Record<string, any>) => number} A comparator function.
 */
export function compareByPropertyWithKeywordFirst(property, keyword) {
  const lowercasedKeyword = keyword?.toLocaleLowerCase();

  return (a, b) => {
    const valueA = a[property] ?? "";
    const valueB = b[property] ?? "";

    // Handle keyword: push it to the beginning
    if (valueA.toLocaleLowerCase() === lowercasedKeyword) return -1;
    if (valueB.toLocaleLowerCase() === lowercasedKeyword) return 1;

    // Handle missing or empty values: push them to the end
    if (!valueA && valueB) return 1;
    if (!valueB && valueA) return -1;

    return valueA.localeCompare(valueB, undefined, { sensitivity: "base" });
  };
}
