
# ServiceSummary

Service summary

## Properties

Name | Type
------------ | -------------
`deployment` | string
`desiredReplicas` | number
`endpoints` | [Array&lt;ServiceEndpoint&gt;](ServiceEndpoint.md)
`name` | string
`replicas` | number
`status` | string

## Example

```typescript
import type { ServiceSummary } from '@zlayer/client'

// TODO: Update the object below with actual values
const example = {
  "deployment": null,
  "desiredReplicas": null,
  "endpoints": null,
  "name": null,
  "replicas": null,
  "status": null,
} satisfies ServiceSummary

console.log(example)

// Convert the instance to a JSON string
const exampleJSON: string = JSON.stringify(example)
console.log(exampleJSON)

// Parse the JSON string back to an object
const exampleParsed = JSON.parse(exampleJSON) as ServiceSummary
console.log(exampleParsed)
```

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


