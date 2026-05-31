# ContainerChangeEntry

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**Kind** | **int32** | &#x60;0&#x60; &#x3D; Modified, &#x60;1&#x60; &#x3D; Added, &#x60;2&#x60; &#x3D; Deleted (Docker&#39;s wire integer). | 
**Path** | **string** | Path inside the container that changed (absolute, e.g. &#x60;/etc/hosts&#x60;). | 

## Methods

### NewContainerChangeEntry

`func NewContainerChangeEntry(kind int32, path string, ) *ContainerChangeEntry`

NewContainerChangeEntry instantiates a new ContainerChangeEntry object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewContainerChangeEntryWithDefaults

`func NewContainerChangeEntryWithDefaults() *ContainerChangeEntry`

NewContainerChangeEntryWithDefaults instantiates a new ContainerChangeEntry object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetKind

`func (o *ContainerChangeEntry) GetKind() int32`

GetKind returns the Kind field if non-nil, zero value otherwise.

### GetKindOk

`func (o *ContainerChangeEntry) GetKindOk() (*int32, bool)`

GetKindOk returns a tuple with the Kind field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetKind

`func (o *ContainerChangeEntry) SetKind(v int32)`

SetKind sets Kind field to given value.


### GetPath

`func (o *ContainerChangeEntry) GetPath() string`

GetPath returns the Path field if non-nil, zero value otherwise.

### GetPathOk

`func (o *ContainerChangeEntry) GetPathOk() (*string, bool)`

GetPathOk returns a tuple with the Path field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetPath

`func (o *ContainerChangeEntry) SetPath(v string)`

SetPath sets Path field to given value.



[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


