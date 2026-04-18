
# DnsStatusResponse

DNS service status response

## Properties

Name | Type
------------ | -------------
`bindAddr` | string
`enabled` | boolean
`port` | number
`serviceCount` | number
`services` | Array&lt;string&gt;
`zone` | string

## Example

```typescript
import type { DnsStatusResponse } from '@zlayer/client'

// TODO: Update the object below with actual values
const example = {
  "bindAddr": null,
  "enabled": null,
  "port": null,
  "serviceCount": null,
  "services": null,
  "zone": null,
} satisfies DnsStatusResponse

console.log(example)

// Convert the instance to a JSON string
const exampleJSON: string = JSON.stringify(example)
console.log(exampleJSON)

// Parse the JSON string back to an object
const exampleParsed = JSON.parse(exampleJSON) as DnsStatusResponse
console.log(exampleParsed)
```

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


