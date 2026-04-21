
# VolumeMountType

Volume mount kind discriminator.  Selects which [`zlayer_spec::StorageSpec`] variant [`VolumeMount`] is translated into by [`build_service_spec`]. When omitted on the wire, defaults to [`VolumeMountType::Bind`] (legacy behavior).

## Properties

Name | Type
------------ | -------------

## Example

```typescript
import type { VolumeMountType } from '@zlayer/api-client'

// TODO: Update the object below with actual values
const example = {
} satisfies VolumeMountType

console.log(example)

// Convert the instance to a JSON string
const exampleJSON: string = JSON.stringify(example)
console.log(exampleJSON)

// Parse the JSON string back to an object
const exampleParsed = JSON.parse(exampleJSON) as VolumeMountType
console.log(exampleParsed)
```

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


