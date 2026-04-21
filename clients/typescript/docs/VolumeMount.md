
# VolumeMount

Volume mount specification.  The `type` field (a Docker-compatible discriminator) selects how `source` is interpreted: - `\"bind\"` (default): `source` is an absolute host path. - `\"volume\"`: `source` is a named-volume identifier. - `\"tmpfs\"`: no `source`; a memory-backed mount is provisioned.

## Properties

Name | Type
------------ | -------------
`readonly` | boolean
`source` | string
`target` | string
`type` | [VolumeMountType](VolumeMountType.md)

## Example

```typescript
import type { VolumeMount } from '@zlayer/api-client'

// TODO: Update the object below with actual values
const example = {
  "readonly": null,
  "source": null,
  "target": null,
  "type": null,
} satisfies VolumeMount

console.log(example)

// Convert the instance to a JSON string
const exampleJSON: string = JSON.stringify(example)
console.log(exampleJSON)

// Parse the JSON string back to an object
const exampleParsed = JSON.parse(exampleJSON) as VolumeMount
console.log(exampleParsed)
```

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


