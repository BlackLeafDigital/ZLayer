
# StreamInfo

Information about a single L4 stream proxy.

## Properties

Name | Type
------------ | -------------
`backendCount` | number
`backends` | [Array&lt;StreamBackendInfo&gt;](StreamBackendInfo.md)
`port` | number
`protocol` | string
`service` | string

## Example

```typescript
import type { StreamInfo } from '@zlayer/client'

// TODO: Update the object below with actual values
const example = {
  "backendCount": null,
  "backends": null,
  "port": null,
  "protocol": null,
  "service": null,
} satisfies StreamInfo

console.log(example)

// Convert the instance to a JSON string
const exampleJSON: string = JSON.stringify(example)
console.log(exampleJSON)

// Parse the JSON string back to an object
const exampleParsed = JSON.parse(exampleJSON) as StreamInfo
console.log(exampleParsed)
```

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


