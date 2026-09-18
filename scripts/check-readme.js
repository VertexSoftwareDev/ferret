/*
 * Checks README.md against README.tr.md.
 *
 * Two versions of the same document drift apart quietly. A section gets added
 * to one and forgotten in the other; a command changes in one and the other
 * keeps telling people to run something that no longer exists. Prose cannot be
 * compared, but structure and commands can, and those are where the damage is.
 *
 *   node scripts/check-readme.js
 */

'use strict';

const fs = require('fs');
const path = require('path');

const root = path.join(__dirname, '..');
const english = read('README.md');
const turkish = read('README.tr.md');

const problems = [];

function read(name) {
  return { name, text: fs.readFileSync(path.join(root, name), 'utf8') };
}

/** Heading levels in order. The words differ between languages; the shape must not. */
function headings(text) {
  return [...text.matchAll(/^(#{1,6}) +(.*)$/gm)].map((match) => ({
    level: match[1].length,
    title: match[2].trim(),
  }));
}

/** Every fenced block, as `{ language, lines }`. */
function blocks(text) {
  return [...text.matchAll(/^```(\w*)\n([\s\S]*?)^```$/gm)].map((match) => ({
    language: match[1],
    lines: match[2].split('\n'),
  }));
}

/**
 * The commands a shell block actually runs.
 *
 * Trailing `#` comments are translated and must differ, so they come off before
 * the comparison — what has to match is the command itself.
 */
function commands(block) {
  return block.lines
    .map((line) => line.replace(/#.*$/, '').trim())
    .filter((line) => line.length > 0);
}

/**
 * The first word of each line of a plain block.
 *
 * In the layout listing that is the directory, and in the self-test capture it
 * is the name of the measurement — in both cases the thing that has to be the
 * same in both files. What follows it is prose, and prose is the point of
 * having two files.
 */
function labels(block) {
  return block.lines
    .map((line) => line.trim().split(/\s+/)[0])
    .filter((label) => label.length > 0);
}

/** Link destinations, which are paths and URLs and so are never translated. */
function links(text) {
  const inline = [...text.matchAll(/\]\(([^)]+)\)/g)].map((match) => match[1]);
  const reference = [...text.matchAll(/^\[[^\]]+\]:\s*(\S+)$/gm)].map((match) => match[1]);
  // The two files point at each other, so those two are expected to differ.
  return [...inline, ...reference].filter(
    (target) => target !== 'README.md' && target !== 'README.tr.md',
  );
}

/**
 * A link's name with the language taken out of it.
 *
 * A screenshot of the Turkish interface belongs in the Turkish file, so those
 * two targets have to differ. Folding them to one name rather than skipping
 * them keeps the check worth running: a Turkish file pointing at the English
 * screenshot is still a mistake, and this still catches it.
 */
function canonical(target) {
  return target.replace(/-(?:en|tr)(\.[a-z0-9]+)$/i, '-<lang>$1');
}

/* ---------------------------------------------------------------- *
 * Structure
 * ---------------------------------------------------------------- */

const englishHeadings = headings(english.text);
const turkishHeadings = headings(turkish.text);

if (englishHeadings.length !== turkishHeadings.length) {
  problems.push(
    `different number of headings: ${english.name} has ${englishHeadings.length}, ` +
      `${turkish.name} has ${turkishHeadings.length}`,
  );
} else {
  englishHeadings.forEach((heading, index) => {
    const other = turkishHeadings[index];
    if (heading.level !== other.level) {
      problems.push(
        `heading ${index + 1} is level ${heading.level} ("${heading.title}") in ` +
          `${english.name} but level ${other.level} ("${other.title}") in ${turkish.name}`,
      );
    }
  });
}

/* ---------------------------------------------------------------- *
 * Code blocks
 * ---------------------------------------------------------------- */

const englishBlocks = blocks(english.text);
const turkishBlocks = blocks(turkish.text);

if (englishBlocks.length !== turkishBlocks.length) {
  problems.push(
    `different number of code blocks: ${english.name} has ${englishBlocks.length}, ` +
      `${turkish.name} has ${turkishBlocks.length}`,
  );
} else {
  englishBlocks.forEach((block, index) => {
    const other = turkishBlocks[index];
    if (block.language !== other.language) {
      problems.push(
        `code block ${index + 1} is \`${block.language || 'plain'}\` in ${english.name} ` +
          `but \`${other.language || 'plain'}\` in ${turkish.name}`,
      );
      return;
    }
    // A shell block is compared by its commands. A plain block is a capture or
    // a directory listing, where the label at the start of each line is the part
    // that must not drift and the rest is description that gets translated.
    const mine = block.language ? commands(block) : labels(block);
    const theirs = block.language ? commands(other) : labels(other);

    if (mine.join('\n') !== theirs.join('\n')) {
      problems.push(
        `code block ${index + 1} differs between ${english.name} and ${turkish.name}:\n` +
          `    ${english.name}: ${JSON.stringify(mine)}\n` +
          `    ${turkish.name}: ${JSON.stringify(theirs)}`,
      );
    }
  });
}

/* ---------------------------------------------------------------- *
 * Links
 * ---------------------------------------------------------------- */

const englishLinks = links(english.text).sort();
const turkishLinks = links(turkish.text).sort();

const englishNames = englishLinks.map(canonical).sort();
const turkishNames = turkishLinks.map(canonical).sort();

if (englishNames.join('\n') !== turkishNames.join('\n')) {
  const missing = englishNames.filter((link) => !turkishNames.includes(link));
  const extra = turkishNames.filter((link) => !englishNames.includes(link));
  if (missing.length) {
    problems.push(`${turkish.name} is missing links: ${missing.join(', ')}`);
  }
  if (extra.length) {
    problems.push(`${turkish.name} has links ${english.name} does not: ${extra.join(', ')}`);
  }
}

// Folding `-en` and `-tr` to one name above means the comparison can no longer
// tell them apart, so each file is asked separately whether its
// language-specific links are in its own language. Without this, the Turkish
// README could show the English screenshot and nothing would object.
for (const { file, suffix } of [
  { file: english, suffix: 'en' },
  { file: turkish, suffix: 'tr' },
]) {
  for (const target of links(file.text)) {
    const found = target.match(/-(en|tr)\.[a-z0-9]+$/i);
    if (found && found[1].toLowerCase() !== suffix) {
      problems.push(`${file.name} links to the ${found[1]} version of ${target}`);
    }
  }
}

/* ---------------------------------------------------------------- *
 * Relative links have to point at something
 * ---------------------------------------------------------------- */

for (const target of new Set([...englishLinks, ...turkishLinks])) {
  if (/^[a-z]+:/i.test(target) || target.startsWith('#')) {
    continue;
  }
  const [file] = target.split('#');
  if (!fs.existsSync(path.join(root, file))) {
    problems.push(`link points at a file that does not exist: ${target}`);
  }
}

/* ---------------------------------------------------------------- */

if (problems.length) {
  console.error('README check failed:\n');
  for (const problem of problems) {
    console.error(`  - ${problem}`);
  }
  process.exit(1);
}

console.log(
  `readme ok: ${englishHeadings.length} sections, ${englishBlocks.length} code blocks, ` +
    `${englishLinks.length} links x 2 languages`,
);
