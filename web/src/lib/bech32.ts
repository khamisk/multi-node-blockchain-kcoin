const CHARSET = 'qpzry9x8gf2tvdw0s3jn54khce6mua7l'
const GENERATORS = [0x3b6a57b2, 0x26508e6d, 0x1ea119fa, 0x3d4233dd, 0x2a1462b3]

function polymod(values: number[]): number {
  let checksum = 1
  for (const value of values) {
    const top = checksum >>> 25
    checksum = ((checksum & 0x1ffffff) << 5) ^ value
    for (let index = 0; index < 5; index += 1) {
      if ((top >>> index) & 1) checksum ^= GENERATORS[index]
    }
  }
  return checksum >>> 0
}

function expandHrp(hrp: string): number[] {
  return [...hrp].map((char) => char.charCodeAt(0) >>> 5)
    .concat([0], [...hrp].map((char) => char.charCodeAt(0) & 31))
}

function convertBits(data: Uint8Array, from: number, to: number): number[] {
  let accumulator = 0
  let bits = 0
  const result: number[] = []
  const mask = (1 << to) - 1
  for (const value of data) {
    accumulator = (accumulator << from) | value
    bits += from
    while (bits >= to) {
      bits -= to
      result.push((accumulator >>> bits) & mask)
    }
  }
  if (bits > 0) result.push((accumulator << (to - bits)) & mask)
  return result
}

function convertWords(words: number[], from: number, to: number, pad: boolean): Uint8Array {
  let accumulator = 0
  let bits = 0
  const result: number[] = []
  const mask = (1 << to) - 1
  for (const word of words) {
    if (word < 0 || (word >>> from) !== 0) throw new Error('Invalid Bech32m data.')
    accumulator = (accumulator << from) | word
    bits += from
    while (bits >= to) {
      bits -= to
      result.push((accumulator >>> bits) & mask)
    }
  }
  if (pad && bits > 0) result.push((accumulator << (to - bits)) & mask)
  if (!pad && (bits >= from || ((accumulator << (to - bits)) & mask) !== 0)) throw new Error('Invalid Bech32m padding.')
  return Uint8Array.from(result)
}

export function encodeBech32m(hrp: string, bytes: Uint8Array): string {
  const words = convertBits(bytes, 8, 5)
  const values = [...expandHrp(hrp), ...words, 0, 0, 0, 0, 0, 0]
  const mod = polymod(values) ^ 0x2bc830a3
  const checksum = Array.from({ length: 6 }, (_, index) => (mod >>> (5 * (5 - index))) & 31)
  return `${hrp}1${[...words, ...checksum].map((value) => CHARSET[value]).join('')}`
}

export function decodeBech32m(value: string, expectedHrp = 'kcoin'): Uint8Array {
  if (value !== value.toLowerCase()) throw new Error('Address must use lowercase Bech32m.')
  const separator = value.lastIndexOf('1')
  if (separator < 1 || separator + 7 > value.length) throw new Error('Malformed Bech32m address.')
  const hrp = value.slice(0, separator)
  if (hrp !== expectedHrp) throw new Error(`Address must start with ${expectedHrp}1.`)
  const values = [...value.slice(separator + 1)].map((character) => {
    const index = CHARSET.indexOf(character)
    if (index < 0) throw new Error('Malformed Bech32m address.')
    return index
  })
  if (polymod([...expandHrp(hrp), ...values]) !== 0x2bc830a3) throw new Error('Invalid Bech32m checksum.')
  return convertWords(values.slice(0, -6), 5, 8, false)
}
