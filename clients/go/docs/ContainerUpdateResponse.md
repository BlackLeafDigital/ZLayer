# ContainerUpdateResponse

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**Warnings** | Pointer to **[]string** | Human-readable warnings emitted by the runtime while applying the update — e.g. &#x60;\&quot;kernel memory limit is deprecated\&quot;&#x60;. | [optional] 

## Methods

### NewContainerUpdateResponse

`func NewContainerUpdateResponse() *ContainerUpdateResponse`

NewContainerUpdateResponse instantiates a new ContainerUpdateResponse object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewContainerUpdateResponseWithDefaults

`func NewContainerUpdateResponseWithDefaults() *ContainerUpdateResponse`

NewContainerUpdateResponseWithDefaults instantiates a new ContainerUpdateResponse object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetWarnings

`func (o *ContainerUpdateResponse) GetWarnings() []string`

GetWarnings returns the Warnings field if non-nil, zero value otherwise.

### GetWarningsOk

`func (o *ContainerUpdateResponse) GetWarningsOk() (*[]string, bool)`

GetWarningsOk returns a tuple with the Warnings field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetWarnings

`func (o *ContainerUpdateResponse) SetWarnings(v []string)`

SetWarnings sets Warnings field to given value.

### HasWarnings

`func (o *ContainerUpdateResponse) HasWarnings() bool`

HasWarnings returns a boolean if a field has been set.


[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


